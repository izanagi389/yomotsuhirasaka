//! 配信API（GraphQL）。02-schema-and-delivery-api.md「配信API」、13-security.md「GraphQL」。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | POST | /ws/{ws}/graphql/queries | クエリを登録する（`{query}`。`register` の範囲のキー） |
//! | GET | /ws/{ws}/graphql?id=…&variables=… | 登録済みのクエリを実行する（`read` の範囲のキー） |
//! | POST | /ws/{ws}/graphql | 同じ（`{id, variables}`）。開発モードでは `{query, variables}` も受け付ける |
//!
//! - APIキーは `Authorization: Bearer` でだけ受け付ける。キーのないリクエストは拒否する（INV-API-01）。
//!   URLのワークスペースとキーのワークスペースが違えば拒否する（INV-GQL-07）。範囲の足りないキーは 403（INV-API-02, INV-GQL-02）。
//! - 本番では登録済みのクエリだけを実行する（INV-GQL-01）。複数のクエリの一括送信は受け付けない（INV-GQL-04）。
//! - 登録のときに、スキーマに対する検証と、深さ・複雑さの上限を確かめる（INV-GQL-06）。データは読まない。
//! - 応答は既定で `Cache-Control: private`（INV-GQL-08）。

use std::collections::HashMap;
use std::sync::Arc;

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
use crate::delivery::queries;
use crate::delivery::schema::{QueryLimits, RequestData};
use crate::workspace_db;

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

/// キーを照合し、回数制限を数える。
async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
    ws: &str,
    scope: &str,
) -> Result<(Uuid, Grant), ApiError> {
    let ws = Uuid::parse_str(ws).map_err(|_| ApiError::not_found())?;
    let grant = api_keys::authenticate(&state.pool, bearer(headers), ws, scope)
        .await?
        .map_err(denied)?;
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
    Path(ws): Path<String>,
    headers: HeaderMap,
    body: Result<Json<Register>, axum::extract::rejection::JsonRejection>,
) -> Result<(StatusCode, Json<Registered>), ApiError> {
    let (ws, grant) = authenticate(&state, &headers, &ws, "register").await?;
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

/// 実行するもの。
struct Execute {
    id: Option<String>,
    query: Option<String>,
    variables: Option<Value>,
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
        &ws,
        &headers,
        Execute {
            id: params.get("id").cloned(),
            query: params.get("query").cloned(),
            variables,
        },
    )
    .await
}

pub async fn post(
    State(state): State<AppState>,
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
        &ws,
        &headers,
        Execute {
            id: text(body.remove("id")),
            query: text(body.remove("query")),
            variables: body.remove("variables").filter(|v| !v.is_null()),
        },
    )
    .await
}

async fn execute(
    state: &AppState,
    ws: &str,
    headers: &HeaderMap,
    req: Execute,
) -> Result<Response, ApiError> {
    let (ws, grant) = authenticate(state, headers, ws, "read").await?;
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
    let schema = schema_for(state, ws).await?;
    let tx = workspace_db::begin_owned(&state.pool, &db).await?;
    let data = Arc::new(RequestData {
        tx: Mutex::new(Some(tx)),
        workspace_id: ws,
        media_base: super::admin_media::media_base(state),
    });
    let request = async_graphql::Request::new(query)
        .variables(async_graphql::Variables::from_json(variables))
        .data(data.clone());
    let response = schema.execute(request).await;
    if let Some(tx) = data.tx.lock().await.take() {
        tx.rollback().await.map_err(anyhow::Error::from)?;
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
    let cache = if shared_cacheable(state, &grant) {
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
