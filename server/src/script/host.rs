//! ホスト関数（04-hooks.md「外部へのアクセス」、Q11）。
//!
//! - 関数が権限として宣言したものだけを、グローバルの `cms` に置く（INV-HOOK-02）。
//! - DBは、関数を動かしているワークスペースのロールに切り替えたトランザクションでだけ読む。他のワークスペースのスキーマには、
//!   DBの権限でも触れられない（INV-HOOK-03、INV-DB-01）。
//! - 待つ時間も、関数の実行時間の上限に含める。
//!
//! - 外部への送信は、外部への送信の共通の仕組み（`egress`）だけを通す。宛先は関数が宣言した `allowedHosts` に限り、そのうえで
//!   内部向けのアドレスの遮断・検査したアドレスへの固定・リダイレクトをたどらないことなどを egress が強制する（INV-HOOK-05）。
//!   宛先や相手の応答の本文はログに出さない。
//!
//! | 権限 | 関数 | 内容 |
//! |---|---|---|
//! | `content.read` | `cms.content.get(id)` | 同じワークスペースの公開中の記事（`{id, contentType, locale, fields}`）。なければ null |
//! | `http.fetch` | `cms.http.fetch(url, {method, headers, body})` | `{status, body}`（本文は1MBまで。3xxもそのまま返す）。失敗は例外 |

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rquickjs::function::Opt;
use rquickjs::{Ctx, Exception, Function, Object, Value as JsValue};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use tokio::runtime::Handle;
use uuid::Uuid;

use super::Permission;
use crate::egress::{Egress, EgressRequest};
use crate::workspace_db::{self, WorkspaceDb};

/// `http.fetch` の応答の本文と、送る本文の大きさの上限（バイト）。
const MAX_FETCH_BODY_BYTES: usize = 1024 * 1024;
/// `http.fetch` で付けられるヘッダーの数の上限。
const MAX_FETCH_HEADERS: usize = 50;

/// 関数を動かすワークスペース。ホスト関数は、このワークスペースのデータだけを読む。
#[derive(Clone)]
pub struct Host {
    pool: PgPool,
    db: WorkspaceDb,
    fetch: Option<Fetch>,
    /// `cms.content.get` で読もうとした記事のID（URL生成関数の依存の記録に使う。05-urls-and-redirects.md、Q39）。
    reads: Arc<Mutex<BTreeSet<Uuid>>>,
}

/// 外部へ送る手段（非同期のフックだけが持つ）。
#[derive(Clone)]
struct Fetch {
    egress: Egress,
    allowed_hosts: Vec<String>,
}

impl Host {
    pub fn new(pool: PgPool, db: WorkspaceDb) -> Self {
        Self {
            pool,
            db,
            fetch: None,
            reads: Arc::default(),
        }
    }

    /// 読んだ記事を新しく数え始める写し（1回の関数の実行の依存を集めるため）。
    pub fn tracking(&self) -> Self {
        Self {
            reads: Arc::default(),
            ..self.clone()
        }
    }

    /// これまでに `cms.content.get` で読もうとした記事のID（見つからなかったものも含む。後から公開されうるため）。
    pub fn reads(&self) -> Vec<Uuid> {
        self.reads
            .lock()
            .map(|r| r.iter().copied().collect())
            .unwrap_or_default()
    }

    /// 外部へ送る手段を付ける（非同期のフック用）。宛先は `allowed_hosts` に限る。
    pub fn with_fetch(self, egress: Egress, allowed_hosts: &[String]) -> Self {
        Self {
            fetch: Some(Fetch {
                egress,
                allowed_hosts: allowed_hosts.iter().map(|h| normalize_host(h)).collect(),
            }),
            ..self
        }
    }

    pub(super) fn without_fetch(self) -> Self {
        Self {
            fetch: None,
            ..self
        }
    }

    pub(super) fn bind(self, handle: Handle) -> Bound {
        Bound { host: self, handle }
    }
}

/// 実行するスレッドから、tokio のランタイムでDBを読むための組。
pub(super) struct Bound {
    host: Host,
    handle: Handle,
}

/// 宣言された権限のホスト関数を、グローバルの `cms` に置く。権限がなければ `cms` も置かない。
pub(super) fn install<'js>(
    ctx: &Ctx<'js>,
    permissions: &[Permission],
    bound: Bound,
    deadline: Instant,
) -> rquickjs::Result<()> {
    if permissions.is_empty() {
        return Ok(());
    }
    let cms = Object::new(ctx.clone())?;
    let Bound { host, handle } = bound;
    if permissions.contains(&Permission::HttpFetch)
        && let Some(fetch) = host.fetch.clone()
    {
        let http = Object::new(ctx.clone())?;
        let handle = handle.clone();
        let send = Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>,
                  url: String,
                  options: Opt<JsValue<'js>>|
                  -> rquickjs::Result<JsValue<'js>> {
                let options = match options.0 {
                    Some(v) if !v.is_undefined() && !v.is_null() => ctx
                        .json_stringify(v)?
                        .map(|s| s.to_string())
                        .transpose()?
                        .and_then(|s| serde_json::from_str::<FetchOptions>(&s).ok())
                        .ok_or_else(|| Exception::throw_type(&ctx, "invalid fetch options"))?,
                    _ => FetchOptions::default(),
                };
                if !fetch.allows(&url) {
                    return Err(Exception::throw_type(
                        &ctx,
                        "this host is not in allowedHosts",
                    ));
                }
                let request = options
                    .into_request(url)
                    .ok_or_else(|| Exception::throw_type(&ctx, "invalid fetch options"))?;
                let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                    return Err(Exception::throw_internal(&ctx, "time limit"));
                };
                match handle.block_on(tokio::time::timeout(left, fetch.egress.send(request))) {
                    Ok(Ok(res)) => {
                        let body = String::from_utf8_lossy(res.body.as_deref().unwrap_or_default())
                            .into_owned();
                        ctx.json_parse(json!({ "status": res.status, "body": body }).to_string())
                    }
                    // 宛先や応答は出さず、種類だけを伝える
                    Ok(Err(e)) => Err(Exception::throw_internal(
                        &ctx,
                        &format!("fetch failed: {}", e.code()),
                    )),
                    Err(_) => Err(Exception::throw_internal(&ctx, "time limit")),
                }
            },
        )?;
        http.set("fetch", send)?;
        cms.set("http", http)?;
    }
    if permissions.contains(&Permission::ContentRead) {
        let content = Object::new(ctx.clone())?;
        let get = Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, id: String| -> rquickjs::Result<JsValue<'js>> {
                let Ok(id) = Uuid::parse_str(&id) else {
                    return Ok(JsValue::new_null(ctx));
                };
                if let Ok(mut reads) = host.reads.lock() {
                    reads.insert(id);
                }
                let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                    return Err(Exception::throw_internal(&ctx, "time limit"));
                };
                let found = handle.block_on(tokio::time::timeout(left, read_entry(&host, id)));
                match found {
                    Ok(Ok(Some(json))) => ctx.json_parse(json),
                    Ok(Ok(None)) => Ok(JsValue::new_null(ctx)),
                    Ok(Err(e)) => {
                        tracing::warn!(workspace_id = %host.db.workspace_id, "cms.content.get failed: {e:#}");
                        Err(Exception::throw_internal(&ctx, "cannot read the entry"))
                    }
                    Err(_) => Err(Exception::throw_internal(&ctx, "time limit")),
                }
            },
        )?;
        content.set("get", get)?;
        cms.set("content", content)?;
    }
    ctx.globals().set("cms", cms)
}

/// 公開中の記事を、公開した版のフィールドで返す（下書きの内容は公開URLに使わない）。
async fn read_entry(host: &Host, id: Uuid) -> anyhow::Result<Option<String>> {
    let mut tx = workspace_db::begin(&host.pool, &host.db).await?;
    let row: Option<(Uuid, String, String, Option<String>)> = sqlx::query_as(
        "SELECT e.id, e.content_type, e.locale, v.fields::text
         FROM entries e JOIN versions v ON v.id = e.published_version_id
         WHERE e.id = $1 AND e.deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    let Some((id, content_type, locale, Some(fields))) = row else {
        return Ok(None);
    };
    let fields: serde_json::Value = serde_json::from_str(&fields)?;
    Ok(Some(
        json!({ "id": id, "contentType": content_type, "locale": locale, "fields": fields })
            .to_string(),
    ))
}

/// `cms.http.fetch` の第2引数。
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct FetchOptions {
    method: Option<String>,
    headers: std::collections::BTreeMap<String, String>,
    body: Option<String>,
}

impl FetchOptions {
    fn into_request(self, url: String) -> Option<EgressRequest> {
        let method = match self
            .method
            .as_deref()
            .map(str::to_ascii_uppercase)
            .as_deref()
        {
            None | Some("GET") => hyper::Method::GET,
            Some("POST") => hyper::Method::POST,
            Some("PUT") => hyper::Method::PUT,
            Some("PATCH") => hyper::Method::PATCH,
            Some("DELETE") => hyper::Method::DELETE,
            Some(_) => return None,
        };
        if self.headers.len() > MAX_FETCH_HEADERS {
            return None;
        }
        let mut headers = Vec::with_capacity(self.headers.len());
        for (name, value) in self.headers {
            let name = hyper::header::HeaderName::from_bytes(name.as_bytes()).ok()?;
            // 宛先と長さは egress が決める
            if name == hyper::header::HOST || name == hyper::header::CONTENT_LENGTH {
                return None;
            }
            headers.push((name, hyper::header::HeaderValue::from_str(&value).ok()?));
        }
        let body = self.body.unwrap_or_default().into_bytes();
        if body.len() > MAX_FETCH_BODY_BYTES {
            return None;
        }
        Some(EgressRequest {
            method,
            url,
            headers,
            body,
            read_body_limit: Some(MAX_FETCH_BODY_BYTES),
        })
    }
}

impl Fetch {
    /// URLのホストが、宣言した宛先のどれかと同じか。
    fn allows(&self, url: &str) -> bool {
        url::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(normalize_host))
            .is_some_and(|h| self.allowed_hosts.contains(&h))
    }
}

/// ホスト名を比べる形にする（小文字、IPv6の角括弧と末尾の点を外す）。
fn normalize_host(h: &str) -> String {
    h.trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_compared_exactly() {
        let f = |hosts: &[&str]| Fetch {
            egress: Egress::from_config(&Default::default()).unwrap(),
            allowed_hosts: hosts.iter().map(|h| normalize_host(h)).collect(),
        };
        let fetch = f(&["API.example.com", "127.0.0.1", "[::1]"]);
        assert!(fetch.allows("https://api.example.com/x"));
        assert!(fetch.allows("http://127.0.0.1:8080/x"));
        assert!(fetch.allows("http://[::1]/x"));
        for url in [
            "https://evil.example.com/",
            "https://api.example.com.evil.test/",
            "https://example.com/",
            "not a url",
            "https://user@evil.test/?api.example.com",
        ] {
            assert!(!fetch.allows(url), "{url}");
        }
    }

    #[test]
    fn fetch_options() {
        let ok = FetchOptions {
            method: Some("post".into()),
            headers: [("content-type".to_owned(), "application/json".to_owned())].into(),
            body: Some("{}".into()),
        };
        assert!(ok.into_request("https://example.com".into()).is_some());
        for bad in [
            FetchOptions {
                method: Some("CONNECT".into()),
                ..Default::default()
            },
            FetchOptions {
                headers: [("host".to_owned(), "x".to_owned())].into(),
                ..Default::default()
            },
            FetchOptions {
                headers: [("bad header".to_owned(), "x".to_owned())].into(),
                ..Default::default()
            },
            FetchOptions {
                body: Some("x".repeat(MAX_FETCH_BODY_BYTES + 1)),
                ..Default::default()
            },
        ] {
            assert!(bad.into_request("https://example.com".into()).is_none());
        }
    }
}
