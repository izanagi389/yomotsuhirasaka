//! 組み込みJS（04-hooks.md、Q10, Q11, Q12, Q89）。
//!
//! テンプレートに書かれ、サーバー管理者がデプロイした関数（M2ではURL生成関数）を、QuickJS-NG（rquickjs）で動かす。
//!
//! - **実行ごとに新しいランタイム**を作り、終わったら捨てる。ランタイムはヒープもグローバル変数も別なので、
//!   他のワークスペースや前の実行の状態は見えない（INV-HOOK-03）。作るのにかかる時間は1ms未満。
//! - **メモリと実行時間の上限**をランタイムに設定する（Q89、INV-HOOK-04）。時間は割り込みで止める（JSからは捕まえられない）。
//!   メモリの上限は、rquickjs の `allocator` / `rust-alloc` 機能を使うと効かなくなるので、それらは使わない。
//! - 実行は tokio の処理を止めないよう、ブロッキング用のスレッドで行う。同時に動かす数にも上限を設ける。
//! - **ホスト関数は、権限として宣言したものだけ**をグローバルに置く（INV-HOOK-02）。宣言していないものは存在しない。
//! - ホスト関数は、関数を動かしているワークスペースのロールに切り替えたトランザクションでだけDBを読む（INV-HOOK-03）。
//! - 関数のソースはJSのまま受け取り、バイトコードは受け取らない（QuickJS-NGは信頼できないバイトコードを守る対象にしていない）。
//!   import は解決しない（モジュールの読み込みの仕組みを設定しない）。
//! - 関数の受け取る値と返す値、例外の文は、記事の内容を含みうるので、ログにも応答にも出さない（原則7）。失敗は種類だけを返す。

mod host;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rquickjs::{Context, Ctx, Function, Module, Runtime, Value as JsValue};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Semaphore;

pub use host::Host;

/// 関数のソースの大きさの上限（バイト）。
pub const MAX_SOURCE_BYTES: usize = 512 * 1024;
/// 関数が返すURLの長さの上限。
const MAX_URL_LEN: usize = 2000;
/// JSのスタックの上限（バイト）。ブロッキング用のスレッドのスタック（2MB）より十分に小さくする。
const MAX_STACK_BYTES: usize = 512 * 1024;

/// 関数が権限として宣言できるホスト関数（04-hooks.md「外部へのアクセス」）。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
pub enum Permission {
    /// 同じワークスペースの公開中の記事を読む（`cms.content.get(id)`）。
    #[serde(rename = "content.read")]
    ContentRead,
}

/// テンプレートに書かれた関数（CLIがTSをJSに変換して入れる。Q49）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Script {
    /// ESモジュールのJSのソース。既定のエクスポートが関数本体。
    pub source: String,
    /// 呼べるホスト関数。ここにないものは呼べない（INV-HOOK-02）。
    #[serde(default)]
    pub permissions: Vec<Permission>,
}

/// 1回の実行の上限（Q89）。
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub memory_bytes: usize,
    pub time: Duration,
}

/// 実行に失敗した理由（中身は持たない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// 関数が例外を投げた（読み込みの失敗、既定のエクスポートがないことを含む）。
    Exception,
    /// 実行時間の上限を超えた。
    TimeLimit,
    /// メモリの上限を超えた。
    MemoryLimit,
    /// 返した値の形が正しくない。
    InvalidResult,
    /// 実行の仕組みの側の失敗（ランタイムを作れないなど）。
    Internal,
}

impl Failure {
    /// APIのエラーメッセージに使う説明（英語）。
    pub fn describe(self) -> &'static str {
        match self {
            Failure::Exception => "the function threw an exception",
            Failure::TimeLimit => "the function exceeded the time limit",
            Failure::MemoryLimit => "the function exceeded the memory limit",
            Failure::InvalidResult => "the function returned an invalid value",
            Failure::Internal => "the function could not be run",
        }
    }
}

/// 組み込みJSの実行の入口（サーバーに1つ）。
#[derive(Clone)]
pub struct Engine {
    limits: Limits,
    running: Arc<Semaphore>,
}

impl Engine {
    pub fn new(limits: Limits) -> Self {
        let parallel = std::thread::available_parallelism().map_or(2, |n| n.get().max(2));
        Self {
            limits,
            running: Arc::new(Semaphore::new(parallel)),
        }
    }

    /// URL生成関数を動かし、公開URLを返す（05-urls-and-redirects.md、Q38）。
    /// `input` は公開する記事（`{id, contentType, locale, slug, fields}`）。
    pub async fn url(&self, script: &Script, host: Host, input: &Value) -> Result<String, Failure> {
        let out = self.call(script, Some(host), input).await?;
        match out {
            Value::String(url) if valid_url(&url) => Ok(url),
            _ => Err(Failure::InvalidResult),
        }
    }

    /// デプロイの検査: ソースを読み込み、既定のエクスポートが関数であることを確かめる（関数は呼ばない）。
    pub async fn check(&self, script: &Script) -> Result<(), Failure> {
        let _permit = self
            .running
            .acquire()
            .await
            .map_err(|_| Failure::Internal)?;
        let (limits, source) = (self.limits, script.source.clone());
        tokio::task::spawn_blocking(move || run(limits, &source, &[], None, None))
            .await
            .map_err(|_| Failure::Internal)?
            .map(|_| ())
    }

    async fn call(
        &self,
        script: &Script,
        host: Option<Host>,
        input: &Value,
    ) -> Result<Value, Failure> {
        let _permit = self
            .running
            .acquire()
            .await
            .map_err(|_| Failure::Internal)?;
        let limits = self.limits;
        let source = script.source.clone();
        let permissions = script.permissions.clone();
        let input = input.to_string();
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let host = host.map(|h| h.bind(handle));
            run(limits, &source, &permissions, host, Some(&input))
        })
        .await
        .map_err(|_| Failure::Internal)?
    }
}

/// 関数が返したURLとして受け付けるか: http(s) の絶対URLで、空白や制御文字を含まず、長すぎない。
fn valid_url(s: &str) -> bool {
    s.len() <= MAX_URL_LEN
        && !s.chars().any(|c| c.is_whitespace() || c.is_control())
        && url::Url::parse(s)
            .is_ok_and(|u| matches!(u.scheme(), "https" | "http") && u.host().is_some())
}

/// 新しいランタイムで、モジュールを読み込み、`input` があれば既定のエクスポートを呼ぶ（ブロッキング用のスレッドで呼ぶ）。
/// 返り値は関数の返した値（`input` がなければ null）。
fn run(
    limits: Limits,
    source: &str,
    permissions: &[Permission],
    host: Option<host::Bound>,
    input: Option<&str>,
) -> Result<Value, Failure> {
    let runtime = Runtime::new().map_err(|_| Failure::Internal)?;
    runtime.set_memory_limit(limits.memory_bytes);
    runtime.set_max_stack_size(MAX_STACK_BYTES);
    let deadline = Instant::now() + limits.time;
    let timed_out = Arc::new(AtomicBool::new(false));
    let flag = timed_out.clone();
    runtime.set_interrupt_handler(Some(Box::new(move || {
        let over = Instant::now() >= deadline;
        if over {
            flag.store(true, Ordering::Relaxed);
        }
        over
    })));
    let context = Context::full(&runtime).map_err(|_| Failure::Internal)?;
    let result = context.with(|ctx| {
        let outcome = (|| -> rquickjs::Result<Value> {
            if let Some(host) = host {
                host::install(&ctx, permissions, host, deadline)?;
            }
            let (module, promise) =
                Module::declare(ctx.clone(), "template-function", source)?.eval()?;
            promise.finish::<()>()?;
            let f: Function = module.get("default")?;
            let Some(input) = input else {
                return Ok(Value::Null);
            };
            let arg = ctx.json_parse(input)?;
            let out: JsValue = f.call((arg,))?;
            Ok(to_json(&ctx, out))
        })();
        outcome.map_err(|e| classify(&ctx, &e))
    });
    match result {
        Err(_) if timed_out.load(Ordering::Relaxed) => Err(Failure::TimeLimit),
        other => other,
    }
}

/// 関数の返した値をJSONにする（JSONにできない値は InvalidResult になるよう、ここでは null にしない）。
fn to_json<'js>(ctx: &Ctx<'js>, v: JsValue<'js>) -> Value {
    if let Some(s) = v.as_string().and_then(|s| s.to_string().ok()) {
        return Value::String(s);
    }
    match ctx.json_stringify(v) {
        Ok(Some(s)) => s
            .to_string()
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// 失敗の種類を決める。例外の文は外に出さず、メモリ不足かどうかの判定にだけ使う。
fn classify(ctx: &Ctx<'_>, e: &rquickjs::Error) -> Failure {
    match e {
        rquickjs::Error::Allocation => Failure::MemoryLimit,
        rquickjs::Error::Exception => {
            let thrown = ctx.catch();
            let out_of_memory = thrown
                .as_exception()
                .and_then(|x| x.message())
                .is_some_and(|m| m == "out of memory");
            if out_of_memory {
                Failure::MemoryLimit
            } else {
                Failure::Exception
            }
        }
        _ => Failure::Exception,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(memory_mb: usize, time_ms: u64) -> Engine {
        Engine::new(Limits {
            memory_bytes: memory_mb * 1024 * 1024,
            time: Duration::from_millis(time_ms),
        })
    }

    fn script(source: &str) -> Script {
        Script {
            source: source.into(),
            permissions: vec![],
        }
    }

    async fn call(e: &Engine, source: &str, input: Value) -> Result<Value, Failure> {
        e.call(&script(source), None, &input).await
    }

    #[tokio::test]
    async fn runs_the_default_export() {
        let e = engine(32, 1000);
        let out = call(
            &e,
            "export default function (x) { return x.locale + '/' + x.slug; }",
            serde_json::json!({"locale": "ja", "slug": "a"}),
        )
        .await;
        assert_eq!(out, Ok(Value::String("ja/a".into())));
    }

    #[tokio::test]
    async fn time_limit_cannot_be_caught() {
        let e = engine(32, 200);
        let started = Instant::now();
        let out = call(
            &e,
            "export default function () { for (;;) { try { for (;;) {} } catch (e) {} } }",
            Value::Null,
        )
        .await;
        assert_eq!(out, Err(Failure::TimeLimit));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn memory_limit_stops_the_function() {
        let e = engine(16, 5000);
        let out = call(
            &e,
            "export default function () { const a = []; for (let i = 0; i < 256; i++) a.push('x'.repeat(1 << 20) + i); return 'done'; }",
            Value::Null,
        )
        .await;
        assert_eq!(out, Err(Failure::MemoryLimit));
    }

    #[tokio::test]
    async fn exceptions_and_bad_modules_fail() {
        let e = engine(32, 1000);
        for source in [
            "export default function () { throw new Error('secret text'); }",
            "export default 1;",
            "export function notDefault() {}",
            "this is not javascript",
            "import x from 'other'; export default function () { return x; }",
        ] {
            assert_eq!(
                call(&e, source, Value::Null).await,
                Err(Failure::Exception),
                "{source}"
            );
        }
    }

    #[tokio::test]
    async fn no_state_is_shared_between_runs() {
        let e = engine(32, 1000);
        let source = "export default function () { const p = globalThis.seen || 'none'; globalThis.seen = 'set'; return p; }";
        for _ in 0..2 {
            assert_eq!(
                call(&e, source, Value::Null).await,
                Ok(Value::String("none".into()))
            );
        }
    }

    #[tokio::test]
    async fn host_functions_are_absent_without_a_host() {
        let e = engine(32, 1000);
        let out = call(
            &e,
            "export default function () { return typeof cms; }",
            Value::Null,
        )
        .await;
        assert_eq!(out, Ok(Value::String("undefined".into())));
    }

    #[tokio::test]
    async fn checks_modules_without_calling() {
        let e = engine(32, 1000);
        assert_eq!(
            e.check(&script("export default function () { for (;;) {} }"))
                .await,
            Ok(())
        );
        assert_eq!(
            e.check(&script("export default 1;")).await,
            Err(Failure::Exception)
        );
        assert_eq!(
            e.check(&script("syntax error here")).await,
            Err(Failure::Exception)
        );
    }

    #[test]
    fn urls() {
        assert!(valid_url("https://example.com/ja/blog/a"));
        assert!(valid_url("http://localhost:3000/a"));
        for bad in [
            "/blog/a",
            "javascript:alert(1)",
            "https://exa mple.com/a",
            "https://example.com/a\n",
            "mailto:a@example.com",
            &format!("https://example.com/{}", "a".repeat(MAX_URL_LEN)),
        ] {
            assert!(!valid_url(bad), "{bad}");
        }
    }
}
