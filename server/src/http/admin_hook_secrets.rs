//! 管理API: フックが読む秘密（04-hooks.md「秘密」）。ワークスペースの管理者とサーバー管理者だけ。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | GET | /api/admin/workspaces/{ws}/hook-secrets | 名前と更新日時の一覧（値は返さない） |
//! | PUT | /api/admin/workspaces/{ws}/hook-secrets/{name} | 登録か置き換え（`{value}`。再認証が必要） |
//! | DELETE | /api/admin/workspaces/{ws}/hook-secrets/{name} | 削除（再認証が必要） |

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use uuid::Uuid;

use super::AppState;
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::access;
use crate::audit::{self, action};
use crate::auth::session::Current;
use crate::hook_secrets::{self, MAX_VALUE_BYTES, Saved, Summary};

fn parse_ws(ws: &str) -> Result<Uuid, ApiError> {
    Uuid::try_parse(ws).map_err(|_| ApiError::not_found())
}

fn invalid(message: &'static str) -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_hook_secret",
        message,
    )
}

async fn may_change(state: &AppState, current: &Current, ws: Uuid) -> Result<(), ApiError> {
    access::workspace_admin(&state.pool, current, ws).await??;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    Ok(())
}

pub async fn list(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<Vec<Summary>>, ApiError> {
    let ws = parse_ws(&ws)?;
    access::workspace_admin(&state.pool, &current, ws).await??;
    Ok(Json(hook_secrets::list(&state.pool, ws).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveSecret {
    value: String,
}

pub async fn save(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, name)): Path<(String, String)>,
    JsonBody(req): JsonBody<SaveSecret>,
) -> Result<StatusCode, ApiError> {
    let ws = parse_ws(&ws)?;
    may_change(&state, &current, ws).await?;
    if !hook_secrets::valid_name(&name) {
        return Err(invalid(
            "The name must start with a letter and use only letters, digits and underscores (64 characters at most).",
        ));
    }
    if req.value.is_empty() || req.value.len() > MAX_VALUE_BYTES {
        return Err(invalid("The value must be 1 to 4096 bytes."));
    }
    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    let saved =
        hook_secrets::save(&mut tx, &state.keys, ws, &name, &req.value, current.user_id).await?;
    if saved == Saved::TooMany {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "too_many_hook_secrets",
            "Remove a secret before adding another one.",
        ));
    }
    audit::record(
        &mut *tx,
        Some(ws),
        Some(current.user_id),
        action::HOOK_SECRET_SAVED,
        &[ws],
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, name)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let ws = parse_ws(&ws)?;
    may_change(&state, &current, ws).await?;
    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    if !hook_secrets::delete(&mut tx, ws, &name).await? {
        return Err(ApiError::not_found());
    }
    audit::record(
        &mut *tx,
        Some(ws),
        Some(current.user_id),
        action::HOOK_SECRET_DELETED,
        &[ws],
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}
