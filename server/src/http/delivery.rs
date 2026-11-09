//! 配信API（GraphQL）。02-schema-and-delivery-api.md「配信API」、13-security.md「GraphQL」。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | POST | /ws/{ws}/graphql/queries | クエリを登録する（`{query}`。`register` の範囲のキー） |
//! | GET | /ws/{ws}/graphql?id=…&variables=… | 登録済みのクエリを実行する（`read` の範囲のキー） |
//! | POST | /ws/{ws}/graphql | 同じ（`{id, variables}`）。開発モードでは `{query, variables}` も受け付ける |
//!
//! - 回答API（M4。`answer`・`answerStream`）は、POST だけで受け付け、`answer` の範囲のキーを求める（お金がかかるため、
//!   `read` のキーでは呼べない）。ほかのフィールドには `read` の範囲を求める。範囲は、実行する文のルートのフィールドで確かめる。
//!   回答の応答は `Cache-Control: private, no-store`。`answerStream`（subscription）は SSE で返す
//!   （GraphQL over SSE の「別々の接続」の形: `event: next` の data に GraphQL の応答、最後に `event: complete`）。
//! - 回答の回数制限（M4、INV-QA-14）: 配信APIの回数制限に加えて、APIキーごとの回答の回数と、訪問者の識別子（`visitorId` の
//!   引数）ごとの回数（1分あたり・1日あたり）を、実行の前に数える。超えたら 429（`answer_rate_limited`・`visitor_rate_limited`）
//!   で、質問の検査・検索・LLMの呼び出しをしない。ストリーミングも SSE を始める前に断る。
//!
//! - APIキーは `Authorization: Bearer` でだけ受け付ける。キーのないリクエストは拒否する（INV-API-01）。
//!   URLのワークスペースとキーのワークスペースが違えば拒否する（INV-GQL-07）。範囲の足りないキーは 403（INV-API-02, INV-GQL-02）。
//! - 本番では登録済みのクエリだけを実行する（INV-GQL-01）。複数のクエリの一括送信は受け付けない（INV-GQL-04）。
//! - 登録のときに、スキーマに対する検証と、深さ・複雑さの上限を確かめる（INV-GQL-06）。データは読まない。
//! - 応答は既定で `Cache-Control: private`（INV-GQL-08）。
//! - プレビュー（M2、INV-PRV-01, 02）: 読み取り用のキーに加えて `X-Yomi-Preview-Token` でプレビュー用トークンを送ると、その記事だけを
//!   下書きで返す（1件を取るクエリだけ）。トークンだけでは使えない（キーの照合が先）。無効なトークンは 401。
//!   トークン付きの応答は `Cache-Control: private, no-store`（共有キャッシュにもブラウザにも残さない）。
//!
//! | GET | /ws/{ws}/preview | プレビュー用トークンの確認（`read` の範囲のキーとトークン）。対象の記事・下書きのURL・有効期限 |

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::OwnedSemaphorePermit;

use super::extract::ClientIp;

use async_graphql::dynamic::Schema;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::Mutex;
use uuid::Uuid;

use super::AppState;
use super::error::ApiError;
use crate::api_keys::{self, Denied, Grant};
use crate::content::applied_schema;
use crate::content::previews;
use crate::content::publishing::Scripts;
use crate::delivery::data;
use crate::delivery::limiter::AnswerLimited;
use crate::delivery::names::{ANSWER_FIELD, ANSWER_STREAM_FIELD};
use crate::delivery::queries;
use crate::delivery::schema::{QueryLimits, RequestData};
use crate::script::Host;
use crate::workspace_db::{self, WorkspaceDb};

/// プレビュー用トークンのヘッダー。
const PREVIEW_HEADER: &str = "x-yomi-preview-token";

fn preview_token(headers: &HeaderMap) -> Option<&str> {
    headers.get(PREVIEW_HEADER)?.to_str().ok()
}

fn invalid_preview_token() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "invalid_preview_token",
        "The preview token is not valid. It may have expired or been revoked.",
    )
}

/// プレビュー用トークンを照合し、対象の記事を下書きから作る（キーの照合の後に呼ぶ）。
async fn preview_entry(
    state: &AppState,
    ws: Uuid,
    db: &WorkspaceDb,
    token: &str,
) -> Result<(data::Entry, String), ApiError> {
    let applied = applied_schema(&state.pool, ws)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let mut tx = workspace_db::begin(&state.pool, db).await?;
    let Some((entry_id, expires_at)) = previews::lookup(&mut tx, token).await? else {
        return Err(invalid_preview_token());
    };
    let scripts = Scripts {
        engine: &state.scripts,
        host: Host::new(state.pool.clone(), db.clone()),
    };
    let entry = data::draft(&mut tx, &applied.manifest, &scripts, entry_id)
        .await?
        .ok_or_else(invalid_preview_token)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok((entry, expires_at))
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

fn denied(d: Denied) -> ApiError {
    match d {
        Denied::Invalid => ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_api_key",
            "A valid API key is required in the Authorization header.",
        ),
        // 他のワークスペースのキーには、そのワークスペースがあるかどうかも知らせない
        Denied::WrongWorkspace => ApiError::not_found(),
        Denied::MissingScope => ApiError::new(
            StatusCode::FORBIDDEN,
            "insufficient_scope",
            "This API key does not allow this operation.",
        ),
    }
}

/// キーを照合し、回数制限を数える。`scopes` のどれかを持つキーだけを通す（呼ぶ側が、操作に要るものを確かめる）。
async fn authenticate(
    state: &AppState,
    ip: IpAddr,
    headers: &HeaderMap,
    ws: &str,
    scopes: &[&str],
) -> Result<(Uuid, Grant), ApiError> {
    let ws = Uuid::parse_str(ws).map_err(|_| ApiError::not_found())?;
    // 無効なキーを送り続ける接続元は、DBに問い合わせる前に断る
    if state.invalid_key_limiter.exceeded(ip) {
        return Err(ApiError::rate_limited(60));
    }
    let grant = match api_keys::authenticate_any(&state.pool, bearer(headers), ws, scopes).await? {
        Ok(grant) => grant,
        Err(d) => {
            if matches!(d, Denied::Invalid) {
                let _ = state.invalid_key_limiter.check(ip);
            }
            return Err(denied(d));
        }
    };
    // 回数制限（INV-GQL-03）。用途で、キーの上限の既定値を分ける（Q153）
    let c = &state.config.delivery;
    let key_limit = if grant.purpose == "server" {
        c.server_key_requests_per_minute
    } else {
        c.build_key_requests_per_minute
    };
    state
        .delivery_limiter
        .check_request(ws, grant.key_id, key_limit, c.workspace_requests_per_minute)
        .map_err(ApiError::rate_limited)?;
    Ok((ws, grant))
}

/// 共有キャッシュに載せてよいか（設定で許し、キーの範囲が公開版の読み取りだけのとき。INV-GQL-08）。
fn shared_cacheable(state: &AppState, grant: &Grant) -> bool {
    state.config.delivery.shared_cache
        && grant.has_scope("read")
        && grant.scopes.iter().all(|s| s == "read")
}

/// クエリの実行枠を取る。少しの間だけ待ち、空かなければ断る。
async fn query_slots(
    state: &AppState,
    ws: Uuid,
) -> Result<(OwnedSemaphorePermit, OwnedSemaphorePermit), ApiError> {
    let per_workspace = state.query_slots.for_workspace(
        ws,
        state.config.delivery.max_concurrent_queries_per_workspace,
    );
    let total = state.query_slots.total.clone();
    let busy = || {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "delivery_busy",
            "Too many queries are running. Try again shortly.",
        )
    };
    let wait = Duration::from_secs(QUERY_SLOT_WAIT_SECONDS);
    let acquire = async {
        let a = per_workspace.acquire_owned().await?;
        let b = total.acquire_owned().await?;
        Ok::<_, tokio::sync::AcquireError>((a, b))
    };
    tokio::time::timeout(wait, acquire)
        .await
        .map_err(|_| busy())?
        .map_err(|_| busy())
}

/// ワークスペースに適用済みのスキーマ。
async fn schema_for(state: &AppState, ws: Uuid) -> Result<Arc<Schema>, ApiError> {
    let applied = applied_schema(&state.pool, ws).await?.ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "schema_not_deployed",
            "No template has been deployed to this workspace yet.",
        )
    })?;
    let limits = QueryLimits {
        depth: state.config.delivery.max_depth,
        complexity: state.config.delivery.max_complexity,
    };
    Ok(state.graphql.get_or_build(
        &applied.template,
        applied.version,
        &applied.manifest,
        state.config.dev_mode,
        limits,
    )?)
}

fn invalid_query(messages: Vec<String>) -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_query",
        format!("The query is not valid: {}", messages.join("; ")),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Register {
    query: String,
}

#[derive(Serialize)]
pub struct Registered {
    id: String,
}

pub async fn register(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Path(ws): Path<String>,
    headers: HeaderMap,
    body: Result<Json<Register>, axum::extract::rejection::JsonRejection>,
) -> Result<(StatusCode, Json<Registered>), ApiError> {
    let (ws, grant) = authenticate(&state, ip, &headers, &ws, &["register"]).await?;
    let Json(req) =
        body.map_err(|_| ApiError::bad_request("Send `{\"query\": \"...\"}` as JSON."))?;
    // 1つの問い合わせだけか、スキーマに合うか、深さ・複雑さの上限。データは読まない
    let schema = schema_for(&state, ws).await?;
    crate::delivery::validate::check(&schema, &req.query)
        .await
        .map_err(invalid_query)?;
    let db = workspace_db::lookup(&state.pool, ws)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let id = queries::register(&mut tx, &req.query, grant.key_id).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok((StatusCode::CREATED, Json(Registered { id })))
}

/// クエリの実行枠が空くのを待つ時間（秒）。
const QUERY_SLOT_WAIT_SECONDS: u64 = 5;

/// 実行するもの。
struct Execute {
    id: Option<String>,
    query: Option<String>,
    variables: Option<Value>,
    /// POST で届いたか（回答APIは POST だけ）。
    post: bool,
}

fn batch_refused() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "batch_not_supported",
        "Send one query per request.",
    )
}

pub async fn get(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Path(ws): Path<String>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let variables = match params.get("variables") {
        Some(v) => Some(
            serde_json::from_str(v)
                .map_err(|_| ApiError::bad_request("`variables` must be JSON."))?,
        ),
        None => None,
    };
    execute(
        &state,
        ip,
        &ws,
        &headers,
        Execute {
            id: params.get("id").cloned(),
            query: params.get("query").cloned(),
            variables,
            post: false,
        },
    )
    .await
}

pub async fn post(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Path(ws): Path<String>,
    headers: HeaderMap,
    body: Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(body) =
        body.map_err(|_| ApiError::bad_request("Send the request as a JSON object."))?;
    let Value::Object(mut body) = body else {
        // 配列（一括送信）は受け付けない（INV-GQL-04）
        return Err(batch_refused());
    };
    let text = |v: Option<Value>| match v {
        Some(Value::String(s)) => Some(s),
        _ => None,
    };
    execute(
        &state,
        ip,
        &ws,
        &headers,
        Execute {
            id: text(body.remove("id")),
            query: text(body.remove("query")),
            variables: body.remove("variables").filter(|v| !v.is_null()),
            post: true,
        },
    )
    .await
}

async fn execute(
    state: &AppState,
    ip: IpAddr,
    ws: &str,
    headers: &HeaderMap,
    req: Execute,
) -> Result<Response, ApiError> {
    let (ws, grant) = authenticate(state, ip, headers, ws, &["read", "answer"]).await?;
    let db = workspace_db::lookup(&state.pool, ws)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let query = match (req.id, req.query) {
        (Some(id), None) => {
            let mut tx = workspace_db::begin(&state.pool, &db).await?;
            let found = queries::lookup(&mut tx, &id).await?;
            tx.commit().await.map_err(anyhow::Error::from)?;
            found.ok_or_else(|| {
                ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "persisted_query_not_found",
                    "No registered query has this ID. Register it with a key that has the register scope.",
                )
            })?
        }
        // 開発モードでだけ、登録していないクエリを実行する（Q115）
        (None, Some(q)) if state.config.dev_mode => q,
        (None, Some(_)) => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "persisted_query_required",
                "Only registered queries can be run. Send the ID of a registered query.",
            ));
        }
        _ => {
            return Err(ApiError::bad_request(
                "Send `id` (the ID of a registered query).",
            ));
        }
    };
    let variables = match req.variables {
        None => Value::Object(Map::new()),
        Some(v @ Value::Object(_)) => v,
        Some(_) => return Err(ApiError::bad_request("`variables` must be a JSON object.")),
    };
    // ルートのフィールドで、要るスコープとメソッドを確かめる（回答は answer の範囲と POST、ほかは read の範囲）
    let shape = crate::delivery::validate::shape(&query)
        .ok_or_else(|| ApiError::bad_request("The query is not valid."))?;
    let is_answer = |r: &String| r == ANSWER_FIELD || r == ANSWER_STREAM_FIELD;
    let answering = shape.answering();
    // 回答のフィールドだけを選ぶ文のほかは、すべて read の範囲を求める（ルートのフィールドが空の文も含む）
    let reading = shape.roots.is_empty() || !shape.roots.iter().all(is_answer);
    let missing = ApiError::new(
        StatusCode::FORBIDDEN,
        "insufficient_scope",
        "This API key does not allow this operation.",
    );
    if (answering && !grant.has_scope("answer")) || (reading && !grant.has_scope("read")) {
        return Err(missing);
    }
    // 回答は1つの文に1回だけ（別名で何度も選んで、1回のリクエストでLLMを何度も呼ばせない。登録のときにも確かめる）
    if answering && !shape.answer_alone() {
        return Err(ApiError::bad_request(
            "Select `answer` or `answerStream` once, without other fields.",
        ));
    }
    if answering && !req.post {
        return Err(ApiError::new(
            StatusCode::METHOD_NOT_ALLOWED,
            "answer_requires_post",
            "Answers are not cacheable. Send the request with POST.",
        ));
    }
    if shape.subscription && reading {
        return Err(ApiError::bad_request(
            "Only the answer stream can be subscribed to.",
        ));
    }
    // 回答の回数制限（13-security.md「チャットボットへの悪用」、INV-QA-14）。質問の検査・検索・LLMより前に数える
    if answering {
        answer_limits(state, ws, grant.key_id, &shape, &variables)?;
    }
    let schema = schema_for(state, ws).await?;
    if shape.subscription {
        return Ok(answer_stream(state, &schema, ws, query, variables));
    }
    let preview = match preview_token(headers) {
        Some(token) => Some(preview_entry(state, ws, &db, token).await?.0),
        None => None,
    };
    let previewing = preview.is_some();
    // 同時に実行するクエリの数を抑える（接続のプールは全体で共有。1つのワークスペースや配信だけで使い切らせない）
    let _slots = if reading {
        Some(query_slots(state, ws).await?)
    } else {
        None
    };
    // 回答だけの文では、リクエストのトランザクションを開かない（回答は自分の短いトランザクションで読む。LLMを待つ間、
    // DBの接続を持ち続けないため）
    let tx = if reading {
        Some(workspace_db::begin_owned(&state.pool, &db).await?)
    } else {
        None
    };
    let data = Arc::new(RequestData {
        tx: Mutex::new(tx),
        workspace_id: ws,
        media_base: super::admin_media::media_base(state),
        preview,
        app: Some(state.clone()),
        db: Some(db.clone()),
        resolved: Default::default(),
        max_resolved: state.config.delivery.max_resolved_entries,
    });
    let request = async_graphql::Request::new(query)
        .variables(async_graphql::Variables::from_json(variables))
        .data(data.clone());
    // 全体の時間の上限。切れたらトランザクションは捨てる（接続は戻る）
    let limit = Duration::from_secs(state.config.delivery.max_query_seconds);
    let Ok(response) = tokio::time::timeout(limit, schema.execute(request)).await else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "query_timeout",
            "The query took too long.",
        ));
    };
    if let Some(tx) = data.tx.lock().await.take() {
        tx.rollback().await.map_err(anyhow::Error::from)?;
    }
    if data.over_limit() {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "query_too_large",
            "The query resolves too many entries. Lower the list limits or the depth of references.",
        ));
    }
    let body = serde_json::to_vec(&response).map_err(anyhow::Error::from)?;
    // 応答の大きさの上限。途中まで返すことはしない
    if body.len() > state.config.delivery.max_response_bytes {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "response_too_large",
            "The response is larger than this server allows. Request fewer entries or fields.",
        ));
    }
    let mut res = (StatusCode::OK, body).into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    // 既定で共有キャッシュに載せない。許すのは、設定で有効にし、公開版の読み取りだけのキーで実行したときだけ（INV-GQL-08）
    let cache = if previewing || answering {
        HeaderValue::from_static("private, no-store")
    } else if shared_cacheable(state, &grant) {
        HeaderValue::from_str(&format!(
            "public, max-age=0, s-maxage={}",
            state.config.delivery.shared_cache_seconds
        ))
        .map_err(anyhow::Error::from)?
    } else {
        HeaderValue::from_static("private, no-cache")
    };
    h.insert(header::CACHE_CONTROL, cache);
    Ok(res)
}

/// 訪問者の識別子の長さの上限（文字）。
const MAX_VISITOR_ID_CHARS: usize = 128;

/// 回答の回数を数える（APIキーごとの回答の回数と、訪問者の識別子ごとの回数）。空の識別子は、渡されていないものとする。
fn answer_limits(
    state: &AppState,
    ws: Uuid,
    key: Uuid,
    shape: &crate::delivery::validate::Shape,
    variables: &Value,
) -> Result<(), ApiError> {
    let invalid = || {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "visitor_id_invalid",
            "`visitorId` must be a string of at most 128 characters.",
        )
    };
    let visitor = shape
        .visitor_id(variables)
        .map_err(|_| invalid())?
        .filter(|v| !v.is_empty());
    if visitor
        .as_ref()
        .is_some_and(|v| v.chars().count() > MAX_VISITOR_ID_CHARS)
    {
        return Err(invalid());
    }
    state
        .delivery_limiter
        .check_answer(ws, key, visitor.as_deref(), &state.config.answer)
        .map_err(|limited| match limited {
            AnswerLimited::Key(retry) => ApiError::too_many(
                "answer_rate_limited",
                "Too many answer requests with this API key. Try again later.",
                retry,
            ),
            AnswerLimited::Visitor(retry) => ApiError::too_many(
                "visitor_rate_limited",
                "Too many questions from this visitor. Try again later.",
                retry,
            ),
        })
}

/// 回答のストリーミング（subscription）を SSE で返す。
fn answer_stream(
    state: &AppState,
    schema: &Schema,
    ws: Uuid,
    query: String,
    variables: Value,
) -> Response {
    use async_graphql::futures_util::{StreamExt as _, stream};
    let data = Arc::new(RequestData {
        tx: Mutex::new(None),
        workspace_id: ws,
        media_base: super::admin_media::media_base(state),
        preview: None,
        app: Some(state.clone()),
        db: None,
        resolved: Default::default(),
        max_resolved: usize::MAX,
    });
    let request = async_graphql::Request::new(query)
        .variables(async_graphql::Variables::from_json(variables))
        .data(data);
    let events = schema
        .execute_stream(request)
        .map(|response| {
            let json = serde_json::to_string(&response)
                .unwrap_or_else(|_| r#"{"errors":[{"message":"internal error"}]}"#.to_owned());
            format!("event: next\ndata: {json}\n\n")
        })
        .chain(stream::once(async {
            "event: complete\ndata: \n\n".to_owned()
        }))
        .map(Ok::<_, std::convert::Infallible>);
    let mut res = Response::new(axum::body::Body::from_stream(events));
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    // 手前のプロキシ（nginx など）に、ためずに流させる
    h.insert(
        header::HeaderName::from_static("x-accel-buffering"),
        HeaderValue::from_static("no"),
    );
    res
}

/// プレビュー用トークンの確認の応答。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewInfo {
    entry_id: Uuid,
    /// 下書きから作った公開URL（作れなければ null）。
    url: Option<String>,
    expires_at: String,
}

/// プレビュー用トークンの確認（サイトのプレビュー用ルートが、クッキーに替える前に呼ぶ）。
pub async fn preview_info(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Path(ws): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let (ws, _) = authenticate(&state, ip, &headers, &ws, &["read"]).await?;
    let db = workspace_db::lookup(&state.pool, ws)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let token = preview_token(&headers).ok_or_else(invalid_preview_token)?;
    let (entry, expires_at) = preview_entry(&state, ws, &db, token).await?;
    let mut res = Json(PreviewInfo {
        entry_id: entry.id,
        url: Some(entry.url).filter(|u| !u.is_empty()),
        expires_at,
    })
    .into_response();
    res.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    Ok(res)
}
