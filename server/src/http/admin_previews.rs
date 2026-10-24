//! 管理API: プレビュー用トークン（03-content-and-editing.md「プレビュー（Q72）」、計画の13番、INV-PRV-01, 02）。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | POST | /api/admin/workspaces/{ws}/entries/{id}/preview-tokens | トークンを発行する（`{ttlSeconds?}`。1〜600、既定は600） |
//! | DELETE | /api/admin/workspaces/{ws}/entries/{id}/preview-tokens/{token} | トークンを失効させる |
//!
//! - 発行と失効は、記事を編集できる人（執筆者以上とサーバー管理者）。トークンの値は発行の応答で一度だけ返す。
//! - 回数が多い操作なので、監査ログには残さない（計画の13番の決定）。
//! - 応答の `url` は、下書きから作った公開URL（プレビューで開くページ。作れなければ null）。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AppState;
use super::admin_entries::{db, editor, invalid, parse, readable};
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::content::previews;
use crate::content::publishing::Scripts;
use crate::delivery::data;
use crate::script::Host;
use crate::workspace_db;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Issue {
    #[serde(default)]
    ttl_seconds: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Issued {
    id: Uuid,
    token: String,
    expires_at: String,
    url: Option<String>,
}

pub async fn issue(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
    JsonBody(req): JsonBody<Issue>,
) -> Result<(StatusCode, Json<Issued>), ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    editor(&state, &current, ws).await?;
    let ttl = req.ttl_seconds.unwrap_or(previews::MAX_TTL_SECONDS);
    if !(1..=previews::MAX_TTL_SECONDS).contains(&ttl) {
        return Err(invalid(vec![format!(
            "ttlSeconds: must be 1 to {}",
            previews::MAX_TTL_SECONDS
        )]));
    }
    let applied = readable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let issued = previews::issue(&mut tx, id, current.user_id, ttl)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let scripts = Scripts {
        engine: &state.scripts,
        host: Host::new(state.pool.clone(), db.clone()),
    };
    let url = data::draft(&mut tx, &applied.manifest, &scripts, id)
        .await?
        .map(|e| e.url)
        .filter(|u| !u.is_empty());
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok((
        StatusCode::CREATED,
        Json(Issued {
            id: issued.id,
            token: issued.token,
            expires_at: issued.expires_at,
            url,
        }),
    ))
}

pub async fn revoke(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id, token)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let (ws, id, token) = (parse(&ws)?, parse(&id)?, parse(&token)?);
    editor(&state, &current, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let done = previews::revoke(&mut tx, id, token).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    if done {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found())
    }
}
