//! 管理API: 配信APIのキー（Q13, Q93, Q153）。ワークスペースの管理者・サーバー管理者（多要素認証）が扱う。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | GET | /api/admin/workspaces/{ws}/api-keys | 有効なキーの一覧（秘密の値は含めない） |
//! | POST | /api/admin/workspaces/{ws}/api-keys | 発行（再認証が必要。秘密の値はこの応答でだけ渡す） |
//! | DELETE | /api/admin/workspaces/{ws}/api-keys/{id} | 失効 |

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AppState;
use super::admin_auth::notify;
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::access;
use crate::api_keys::{self, PURPOSES, SCOPES};
use crate::audit::{self, action};
use crate::mail::notify::Event;

const MAX_NAME_CHARS: usize = 100;

fn parse_id(raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|_| ApiError::not_found())
}

pub async fn list(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<Vec<api_keys::Summary>>, ApiError> {
    let ws = parse_id(&ws)?;
    access::workspace_admin(&state.pool, &current, ws).await??;
    Ok(Json(api_keys::list(&state.pool, ws).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateKey {
    name: Option<String>,
    scopes: Vec<String>,
    /// "build"（既定）か "server"。回数制限の既定値だけに影響する（Q153）。
    purpose: Option<String>,
    /// RFC 3339 の時刻。省略すると期限なし。
    expires_at: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Created {
    id: Uuid,
    /// 秘密の値。この応答でだけ渡す（INV-KEY-01）。
    key: String,
    name: String,
    scopes: Vec<String>,
    purpose: String,
    expires_at: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    JsonBody(req): JsonBody<CreateKey>,
) -> Result<(StatusCode, Json<Created>), ApiError> {
    let ws = parse_id(&ws)?;
    access::workspace_admin(&state.pool, &current, ws).await??;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    let name = req.name.as_deref().unwrap_or("API key").trim().to_owned();
    if name.is_empty()
        || name.chars().count() > MAX_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        return Err(ApiError::bad_request(format!(
            "`name` must be 1 to {MAX_NAME_CHARS} characters without control characters."
        )));
    }
    let mut scopes: Vec<String> = Vec::new();
    for scope in req.scopes {
        if !SCOPES.contains(&scope.as_str()) {
            return Err(ApiError::bad_request(
                "`scopes` may contain read and register.",
            ));
        }
        if !scopes.contains(&scope) {
            scopes.push(scope);
        }
    }
    if scopes.is_empty() {
        return Err(ApiError::bad_request("`scopes` must not be empty."));
    }
    let purpose = req.purpose.unwrap_or_else(|| "build".to_owned());
    if !PURPOSES.contains(&purpose.as_str()) {
        return Err(ApiError::bad_request("`purpose` must be build or server."));
    }
    if let Some(at) = &req.expires_at {
        if !crate::timefmt::is_rfc3339(at) {
            return Err(ApiError::bad_request(
                "`expiresAt` must be an RFC 3339 time.",
            ));
        }
        let future = sqlx::query_scalar::<_, bool>("SELECT $1::timestamptz > now()")
            .bind(at)
            .fetch_one(&state.pool)
            .await;
        match future {
            Ok(true) => {}
            Ok(false) => return Err(ApiError::bad_request("`expiresAt` must be in the future.")),
            Err(e)
                if e.as_database_error()
                    .is_some_and(|d| d.code().is_some_and(|c| c == "22007" || c == "22008")) =>
            {
                return Err(ApiError::bad_request(
                    "`expiresAt` must be an RFC 3339 time.",
                ));
            }
            Err(e) => return Err(anyhow::Error::from(e).into()),
        }
    }

    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    let issued = api_keys::issue(
        &mut *tx,
        ws,
        &name,
        &scopes,
        &purpose,
        current.user_id,
        req.expires_at.as_deref(),
    )
    .await?;
    audit::record(
        &mut *tx,
        Some(ws),
        Some(current.user_id),
        action::API_KEY_ISSUED,
        &[issued.id],
    )
    .await?;
    notify(&state, &mut *tx, &current, Event::ApiKeyIssued).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok((
        StatusCode::CREATED,
        Json(Created {
            id: issued.id,
            key: issued.secret,
            name,
            scopes,
            purpose,
            expires_at: issued.expires_at,
        }),
    ))
}

pub async fn revoke(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (ws, id) = (parse_id(&ws)?, parse_id(&id)?);
    access::workspace_admin(&state.pool, &current, ws).await??;
    if !api_keys::revoke(&state.pool, ws, id).await? {
        return Err(ApiError::not_found());
    }
    audit::record(
        &state.pool,
        Some(ws),
        Some(current.user_id),
        action::API_KEY_REVOKED,
        &[id],
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
