//! 管理API: CLI用の個人トークン（Q91, Q119）。どれも管理画面（Cookieのセッション）からだけ使える。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | GET | /api/admin/me/tokens | 本人の有効なトークンの一覧（秘密の値は含めない） |
//! | POST | /api/admin/me/tokens | 発行（再認証が必要。秘密の値はこの応答でだけ渡す） |
//! | DELETE | /api/admin/me/tokens/{id} | 失効 |

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AppState;
use super::admin_auth::notify;
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody, require_cookie_session};
use crate::access;
use crate::audit::{self, action};
use crate::auth::personal_tokens::{self, ExpiryError, SCOPES};
use crate::mail::notify::Event;

/// 有効期限を指定しないときの日数。
const DEFAULT_LIFETIME_DAYS: i32 = 90;
const MAX_NAME_CHARS: usize = 100;

pub async fn list(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
) -> Result<Json<Vec<personal_tokens::Summary>>, ApiError> {
    require_cookie_session(&current)?;
    Ok(Json(
        personal_tokens::list(&state.pool, current.user_id).await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateToken {
    name: Option<String>,
    scopes: Vec<String>,
    #[serde(default)]
    workspace_ids: Vec<Uuid>,
    /// RFC 3339 の時刻。省略すると90日後。最長365日。
    expires_at: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Created {
    id: Uuid,
    /// 秘密の値。この応答でだけ渡す（INV-KEY-01）。
    token: String,
    name: String,
    scopes: Vec<String>,
    workspace_ids: Vec<Uuid>,
    expires_at: String,
}

pub async fn create(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    JsonBody(req): JsonBody<CreateToken>,
) -> Result<(StatusCode, Json<Created>), ApiError> {
    require_cookie_session(&current)?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    let name = req
        .name
        .as_deref()
        .unwrap_or("Personal token")
        .trim()
        .to_owned();
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
                "`scopes` may contain read, write and deploy.",
            ));
        }
        if !scopes.contains(&scope) {
            scopes.push(scope);
        }
    }
    if scopes.is_empty() {
        return Err(ApiError::bad_request("`scopes` must not be empty."));
    }
    // デプロイの範囲は、サーバー管理者だけが付けられる（Q119）
    if scopes.iter().any(|s| s == "deploy") && !current.is_server_admin {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "deploy_scope_forbidden",
            "Only server administrators can create tokens with the deploy scope.",
        ));
    }
    // デプロイの範囲は、ほかの範囲と混ぜられない（漏れたときの影響を分ける）
    let deploy = scopes.iter().any(|s| s == "deploy");
    if deploy && scopes.len() > 1 {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "deploy_scope_exclusive",
            "The deploy scope cannot be combined with other scopes.",
        ));
    }
    // 選べるのは、本人が所属するワークスペースだけ（所属していなければ、存在しないものと同じ）
    let mut workspaces: Vec<Uuid> = Vec::new();
    for ws in req.workspace_ids {
        if access::role_in(&state.pool, ws, current.user_id)
            .await?
            .is_none()
        {
            return Err(ApiError::not_found());
        }
        if !workspaces.contains(&ws) {
            workspaces.push(ws);
        }
    }
    let expires_at = match req.expires_at {
        Some(at) => at,
        None => sqlx::query_scalar(
            r#"SELECT to_char((now() + make_interval(days => $1)) AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"')"#,
        )
        .bind(if deploy {
            personal_tokens::MAX_DEPLOY_LIFETIME_DAYS
        } else {
            DEFAULT_LIFETIME_DAYS
        })
        .fetch_one(&state.pool)
        .await
        .map_err(anyhow::Error::from)?,
    };
    match personal_tokens::check_expiry(&state.pool, &expires_at).await? {
        Ok(()) => {}
        Err(ExpiryError::Invalid) => {
            return Err(ApiError::bad_request(
                "`expiresAt` must be an RFC 3339 time.",
            ));
        }
        Err(ExpiryError::OutOfRange) => {
            return Err(ApiError::bad_request(format!(
                "`expiresAt` must be in the future and within {} days.",
                personal_tokens::MAX_LIFETIME_DAYS
            )));
        }
    }
    if deploy && !personal_tokens::within_deploy_lifetime(&state.pool, &expires_at).await? {
        return Err(ApiError::bad_request(format!(
            "`expiresAt` of a token with the deploy scope must be within {} days.",
            personal_tokens::MAX_DEPLOY_LIFETIME_DAYS
        )));
    }

    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    let issued = personal_tokens::issue(
        &mut *tx,
        current.user_id,
        &name,
        &scopes,
        &workspaces,
        &expires_at,
    )
    .await?;
    audit::record(
        &mut *tx,
        None,
        Some(current.user_id),
        action::PERSONAL_TOKEN_ISSUED,
        &[issued.id],
    )
    .await?;
    notify(&state, &mut *tx, &current, Event::PersonalTokenIssued).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok((
        StatusCode::CREATED,
        Json(Created {
            id: issued.id,
            token: issued.secret,
            name,
            scopes,
            workspace_ids: workspaces,
            expires_at: issued.expires_at,
        }),
    ))
}

pub async fn revoke(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    require_cookie_session(&current)?;
    let id = Uuid::parse_str(&id).map_err(|_| ApiError::not_found())?;
    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    if !personal_tokens::revoke(&mut *tx, current.user_id, id).await? {
        return Err(ApiError::not_found());
    }
    audit::record(
        &mut *tx,
        None,
        Some(current.user_id),
        action::PERSONAL_TOKEN_REVOKED,
        &[id],
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}
