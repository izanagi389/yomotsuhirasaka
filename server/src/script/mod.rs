//! 組み込みJS（04-hooks.md、Q10, Q11, Q12, Q89）。
//!
//! テンプレートに書かれ、サーバー管理者がデプロイした関数（URL生成関数とフック）を、QuickJS-NG（rquickjs）で動かす。
//!
//! - **実行ごとに新しいランタイム**を作り、終わったら捨てる。ランタイムはヒープもグローバル変数も別なので、
//!   他のワークスペースや前の実行の状態は見えない（INV-HOOK-03）。作るのにかかる時間は1ms未満。
//! - **メモリと実行時間の上限**をランタイムに設定する（Q89、INV-HOOK-04）。時間は割り込みで止める（JSからは捕まえられない）。
//!   メモリの上限は、rquickjs の `allocator` / `rust-alloc` 機能を使うと効かなくなるので、それらは使わない。
//! - 実行は tokio の処理を止めないよう、ブロッキング用のスレッドで行う。同時に動かす数にも上限を設ける
//!   （同期の実行と非同期のフックで別に数え、時間のかかる非同期のフックが保存や公開を待たせないようにする）。
//! - 同期の実行（URL生成関数、保存前・公開前のフック）と非同期のフックで、実行時間の上限を分ける（Q12）。
//!   外部への送信（`http.fetch`）は非同期のフックにだけ置く（INV-HOOK-01）。
//! - **ホスト関数は、権限として宣言したものだけ**をグローバルに置く（INV-HOOK-02）。宣言していないものは存在しない。
//! - ホスト関数は、関数を動かしているワークスペースのロールに切り替えたトランザクションでだけDBを読む（INV-HOOK-03）。
//! - 関数のソースはJSのまま受け取り、バイトコードは受け取らない（QuickJS-NGは信頼できないバイトコードを守る対象にしていない）。
//!   import は解決しない（モジュールの読み込みの仕組みを設定しない）。
//! - 関数の受け取る値と返す値、例外の文は、記事の内容を含みうるので、ログにも応答にも出さない（原則7）。失敗は種類だけを返す。

mod host;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rquickjs::{Context, Ctx, Function, Module, Object, Runtime, Value as JsValue};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::Semaphore;

pub use host::Host;

/// 本文の変換器（`packages/converter` をビルドしたもの。`npm run sync:server -w @yomotsuhirasaka/converter` で更新する）。
pub const CONVERTER_SOURCE: &str = include_str!("converter.js");
/// 変換器の版（`packages/converter` の `VERSION`。テストで一致を確かめる）。版に保存し、変わったら作り直す（Q58）。
pub const CONVERTER_VERSION: &str = "1";

/// カスタムノードの変換関数（変換器に渡す）。
#[derive(Debug, Clone)]
pub struct CustomNodeScripts {
    pub name: String,
    pub inline: bool,
    pub to_markdown: String,
    pub to_html: Option<String>,
}

/// 本文のフィールドを変換した結果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rendered {
    pub markdown: String,
    pub html: String,
}

/// 関数のソースの大きさの上限（バイト）。
pub const MAX_SOURCE_BYTES: usize = 512 * 1024;
/// 関数が返すURLの長さの上限。
const MAX_URL_LEN: usize = 2000;
/// 公開前のフックが返せる理由の数と、1つの理由の長さの上限。
const MAX_HOOK_ERRORS: usize = 20;
const MAX_HOOK_MESSAGE_CHARS: usize = 500;
/// JSのスタックの上限（バイト）。ブロッキング用のスレッドのスタック（2MB）より十分に小さくする。
const MAX_STACK_BYTES: usize = 512 * 1024;

/// 関数が権限として宣言できるホスト関数（04-hooks.md「外部へのアクセス」）。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
pub enum Permission {
    /// 同じワークスペースの公開中の記事を読む（`yomi.content.get(id)`）。
    #[serde(rename = "content.read")]
    ContentRead,
    /// 外部へHTTPで送る（`yomi.http.fetch(url, options)`）。非同期のフックだけ。宛先は `allowedHosts` に限る。
    #[serde(rename = "http.fetch")]
    HttpFetch,
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
    /// `http.fetch` で送ってよいホスト名かIPアドレス。ここにあっても、内部向けのアドレスの遮断などの
    /// 外部への送信の検査は外れない（13-security.md「外部への送信」、INV-HOOK-05）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_hosts: Vec<String>,
}

/// 1回の実行の上限（Q89）。
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub memory_bytes: usize,
    /// 同期の実行（URL生成関数、保存前・公開前のフック）。
    pub time: Duration,
    /// 非同期のフック。
    pub async_time: Duration,
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
    /// ジョブの失敗の記録や応答に使う種類。
    pub fn code(self) -> &'static str {
        match self {
            Failure::Exception => "hook_exception",
            Failure::TimeLimit => "hook_time_limit",
            Failure::MemoryLimit => "hook_memory_limit",
            Failure::InvalidResult => "hook_invalid_result",
            Failure::Internal => "hook_internal",
        }
    }

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
    running_async: Arc<Semaphore>,
}

/// 同期か非同期か（上限の時間と、同時に動かす数の数え方が違う）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Sync,
    Async,
}

impl Engine {
    pub fn new(limits: Limits) -> Self {
        let parallel = std::thread::available_parallelism().map_or(2, |n| n.get().max(2));
        Self {
            limits,
            running: Arc::new(Semaphore::new(parallel)),
            running_async: Arc::new(Semaphore::new((parallel / 2).max(1))),
        }
    }

    /// URL生成関数を動かし、公開URLを返す（05-urls-and-redirects.md、Q38）。
    /// `input` は公開する記事（`{id, contentType, locale, slug, fields}`）。
    pub async fn url(&self, script: &Script, host: Host, input: &Value) -> Result<String, Failure> {
        let out = self.call(Mode::Sync, script, Some(host), input).await?;
        match out {
            Value::String(url) if valid_url(&url) => Ok(url),
            _ => Err(Failure::InvalidResult),
        }
    }

    /// 保存前のフック。直すフィールド（`{fields: {...}}` の中身）を返す。直さなければ空。
    pub async fn before_save(
        &self,
        script: &Script,
        host: Host,
        input: &Value,
    ) -> Result<Map<String, Value>, Failure> {
        parse_patch(self.call(Mode::Sync, script, Some(host), input).await?)
    }

    /// 公開前のフック。公開を止める理由（`{errors: [{field?, message}]}`）を、`field: message` の形で返す。
    pub async fn before_publish(
        &self,
        script: &Script,
        host: Host,
        input: &Value,
    ) -> Result<Vec<String>, Failure> {
        parse_errors(self.call(Mode::Sync, script, Some(host), input).await?)
    }

    /// 本文のフィールド（名前 → TiptapのJSON）を、Markdown と HTML に変換する。HTMLは無害化する前のもの。
    pub async fn render(
        &self,
        nodes: Vec<CustomNodeScripts>,
        docs: &Map<String, Value>,
    ) -> Result<std::collections::BTreeMap<String, Rendered>, Failure> {
        if docs.is_empty() {
            return Ok(Default::default());
        }
        let _permit = self
            .running
            .acquire()
            .await
            .map_err(|_| Failure::Internal)?;
        let limits = self.limits_for(Mode::Sync);
        let docs = Value::Object(docs.clone()).to_string();
        let out = tokio::task::spawn_blocking(move || run_render(limits, &nodes, &docs))
            .await
            .map_err(|_| Failure::Internal)??;
        serde_json::from_value(out).map_err(|_| Failure::InvalidResult)
    }

    /// 非同期のフック（保存後・公開後・非公開の後）。返した値は使わない。
    pub async fn after(&self, script: &Script, host: Host, input: &Value) -> Result<(), Failure> {
        self.call(Mode::Async, script, Some(host), input)
            .await
            .map(|_| ())
    }

    /// デプロイの検査: ソースを読み込み、既定のエクスポートが関数であることを確かめる（関数は呼ばない）。
    pub async fn check(&self, script: &Script) -> Result<(), Failure> {
        let _permit = self
            .running
            .acquire()
            .await
            .map_err(|_| Failure::Internal)?;
        let (limits, source) = (self.limits_for(Mode::Sync), script.source.clone());
        tokio::task::spawn_blocking(move || run(limits, &source, &[], None, None))
            .await
            .map_err(|_| Failure::Internal)?
            .map(|_| ())
    }

    /// 実行の上限（`run` には time だけを使わせる）。
    fn limits_for(&self, mode: Mode) -> Limits {
        let time = match mode {
            Mode::Sync => self.limits.time,
            Mode::Async => self.limits.async_time,
        };
        Limits {
            time,
            ..self.limits
        }
    }

    async fn call(
        &self,
        mode: Mode,
        script: &Script,
        host: Option<Host>,
        input: &Value,
    ) -> Result<Value, Failure> {
        let semaphore = match mode {
            Mode::Sync => &self.running,
            Mode::Async => &self.running_async,
        };
        let _permit = semaphore.acquire().await.map_err(|_| Failure::Internal)?;
        let limits = self.limits_for(mode);
        // 同期の実行には、外部へ送る手段を渡さない（デプロイの検査に加えた二重の守り。INV-HOOK-01）
        let host = match mode {
            Mode::Sync => host.map(Host::without_fetch),
            Mode::Async => host,
        };
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

/// 保存前のフックの返した値から、直すフィールドを取り出す（`{fields: {...}}`、null、`{}`）。
fn parse_patch(out: Value) -> Result<Map<String, Value>, Failure> {
    match out {
        Value::Null => Ok(Map::new()),
        Value::Object(mut out) => match out.remove("fields") {
            None | Some(Value::Null) if out.is_empty() => Ok(Map::new()),
            Some(Value::Object(fields)) if out.is_empty() => Ok(fields),
            _ => Err(Failure::InvalidResult),
        },
        _ => Err(Failure::InvalidResult),
    }
}

/// 公開前のフックの返した値から、公開を止める理由を `field: message` の形で取り出す。
fn parse_errors(out: Value) -> Result<Vec<String>, Failure> {
    let errors = match out {
        Value::Null => return Ok(Vec::new()),
        Value::Object(mut out) => match out.remove("errors") {
            None | Some(Value::Null) if out.is_empty() => return Ok(Vec::new()),
            Some(Value::Array(errors)) if out.is_empty() => errors,
            _ => return Err(Failure::InvalidResult),
        },
        _ => return Err(Failure::InvalidResult),
    };
    if errors.len() > MAX_HOOK_ERRORS {
        return Err(Failure::InvalidResult);
    }
    errors
        .iter()
        .map(|e| {
            let field = match e.get("field") {
                None | Some(Value::Null) => None,
                Some(Value::String(f)) => Some(f.chars().take(64).collect::<String>()),
                Some(_) => return Err(Failure::InvalidResult),
            };
            let message = e
                .get("message")
                .and_then(Value::as_str)
                .filter(|m| !m.trim().is_empty())
                .ok_or(Failure::InvalidResult)?;
            let message: String = message.chars().take(MAX_HOOK_MESSAGE_CHARS).collect();
            Ok(match field {
                Some(f) => format!("{f}: {message}"),
                None => message,
            })
        })
        .collect()
}

/// 関数が返したURLとして受け付けるか: http(s) の絶対URLで、空白や制御文字を含まず、長すぎない。
fn valid_url(s: &str) -> bool {
    s.len() <= MAX_URL_LEN
        && !s.chars().any(|c| c.is_whitespace() || c.is_control())
        && url::Url::parse(s)
            .is_ok_and(|u| matches!(u.scheme(), "https" | "http") && u.host().is_some())
}

/// 新しいランタイムを上限付きで作り、`f` を動かす（ブロッキング用のスレッドで呼ぶ）。
/// `f` には、上限の時刻（ホスト関数が待つ時間の上限に使う）を渡す。
fn execute<F>(limits: Limits, f: F) -> Result<Value, Failure>
where
    F: for<'js> FnOnce(&Ctx<'js>, Instant) -> rquickjs::Result<Value>,
{
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
    let result = context.with(|ctx| f(&ctx, deadline).map_err(|e| classify(&ctx, &e)));
    match result {
        Err(_) if timed_out.load(Ordering::Relaxed) => Err(Failure::TimeLimit),
        other => other,
    }
}

/// モジュールを読み込み、既定のエクスポートの関数を返す。
fn load_default<'js>(ctx: &Ctx<'js>, name: &str, source: &str) -> rquickjs::Result<Function<'js>> {
    let (module, promise) = Module::declare(ctx.clone(), name, source)?.eval()?;
    promise.finish::<()>()?;
    module.get("default")
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
    execute(limits, |ctx, deadline| {
        if let Some(host) = host {
            host::install(ctx, permissions, host, deadline)?;
        }
        let f = load_default(ctx, "template-function", source)?;
        let Some(input) = input else {
            return Ok(Value::Null);
        };
        let arg = ctx.json_parse(input)?;
        let out: JsValue = f.call((arg,))?;
        Ok(to_json(ctx, out))
    })
}

/// 変換器で本文のフィールドを変換する（ブロッキング用のスレッドで呼ぶ）。
fn run_render(limits: Limits, nodes: &[CustomNodeScripts], docs: &str) -> Result<Value, Failure> {
    execute(limits, |ctx, _| {
        let (converter, promise) =
            Module::declare(ctx.clone(), "converter", CONVERTER_SOURCE)?.eval()?;
        promise.finish::<()>()?;
        let render: Function = converter.get("renderFields")?;
        let registry = Object::new(ctx.clone())?;
        for (i, n) in nodes.iter().enumerate() {
            let entry = Object::new(ctx.clone())?;
            entry.set(
                "toMarkdown",
                load_default(ctx, &format!("node-{i}-markdown"), &n.to_markdown)?,
            )?;
            if let Some(html) = &n.to_html {
                entry.set(
                    "toHtml",
                    load_default(ctx, &format!("node-{i}-html"), html)?,
                )?;
            }
            entry.set("inline", n.inline)?;
            registry.set(n.name.as_str(), entry)?;
        }
        let docs = ctx.json_parse(docs)?;
        let out: JsValue = render.call((docs, registry))?;
        Ok(to_json(ctx, out))
    })
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
            async_time: Duration::from_millis(time_ms),
        })
    }

    fn script(source: &str) -> Script {
        Script {
            source: source.into(),
            permissions: vec![],
            allowed_hosts: vec![],
        }
    }

    async fn call(e: &Engine, source: &str, input: Value) -> Result<Value, Failure> {
        e.call(Mode::Sync, &script(source), None, &input).await
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
            "export default function () { return typeof yomi; }",
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
    fn converter_version_matches_the_package() {
        assert!(
            CONVERTER_SOURCE.contains(&format!("export const VERSION = \"{CONVERTER_VERSION}\";")),
            "update CONVERTER_VERSION to the converter's VERSION"
        );
    }

    #[tokio::test]
    async fn renders_with_custom_nodes() {
        let e = engine(32, 2000);
        let docs: Map<String, Value> = serde_json::from_str(
            r#"{"body": {"type": "doc", "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "<b>x</b>"}]},
                {"type": "callout", "attrs": {"label": "L"}}
            ]}}"#,
        )
        .unwrap();
        let nodes = vec![CustomNodeScripts {
            name: "callout".into(),
            inline: false,
            to_markdown: "export default function (n) { return '> ' + n.attrs.label; }".into(),
            to_html: Some(
                "export default function (n) { return '<aside>' + n.attrs.label + '</aside>'; }"
                    .into(),
            ),
        }];
        let out = e.render(nodes.clone(), &docs).await.unwrap();
        assert_eq!(out["body"].markdown, "\\<b\\>x\\</b\\>\n\n> L");
        assert_eq!(
            out["body"].html,
            "<p>&#60;b&#62;x&#60;/b&#62;</p>\n<aside>L</aside>"
        );
        let throwing = vec![CustomNodeScripts {
            to_markdown: "export default function () { throw new Error('x'); }".into(),
            ..nodes[0].clone()
        }];
        assert_eq!(e.render(throwing, &docs).await, Err(Failure::Exception));
    }

    #[test]
    fn hook_results() {
        use serde_json::json;
        assert_eq!(parse_patch(Value::Null), Ok(Map::new()));
        assert_eq!(parse_patch(json!({})), Ok(Map::new()));
        assert_eq!(
            parse_patch(json!({"fields": {"a": 1}})),
            Ok(json!({"a": 1}).as_object().unwrap().clone())
        );
        for bad in [
            json!({"fields": 1}),
            json!({"fields": {}, "x": 1}),
            json!("x"),
            json!([]),
        ] {
            assert_eq!(
                parse_patch(bad.clone()),
                Err(Failure::InvalidResult),
                "{bad}"
            );
        }
        assert_eq!(parse_errors(json!({"errors": []})), Ok(vec![]));
        assert_eq!(
            parse_errors(
                json!({"errors": [{"field": "title", "message": "too long"}, {"message": "no"}]})
            ),
            Ok(vec!["title: too long".to_owned(), "no".to_owned()])
        );
        for bad in [
            json!({"errors": [{"field": "title"}]}),
            json!({"errors": [{"message": " "}]}),
            json!({"errors": [{"field": 1, "message": "x"}]}),
            json!({"errors": "x"}),
            json!({"errors": [], "x": 1}),
            json!({"errors": vec![json!({"message": "x"}); MAX_HOOK_ERRORS + 1]}),
        ] {
            assert_eq!(
                parse_errors(bad.clone()),
                Err(Failure::InvalidResult),
                "{bad}"
            );
        }
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
