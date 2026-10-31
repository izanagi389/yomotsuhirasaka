//! 管理API: 編集者向けのAI支援（10-llm-and-budget.md「編集者向けのAI支援」、Q69, Q70、M4）。
//!
//! | メソッド | パス | 内容 | 誰が |
//! |---|---|---|---|
//! | GET | /api/admin/workspaces/{ws}/assist-settings | 支援を使うか（`enabled`） | 重い権限 manageLlm を持つ人・サーバー管理者 |
//! | PUT | /api/admin/workspaces/{ws}/assist-settings | 同じく、変える | 同上（再認証） |
//!
//! - 有効にすると、公開前の下書きが外部のLLMの提供会社に送られる（管理画面で注意を出す。Q177）。
//! - 変更は監査ログに残す（詳細は変えた後の値）。

use axum::Json;
use axum::extract::{Path, State};

use super::AppState;
use super::admin_entries::{db, parse};
use super::admin_publishing::holder;
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::access::Permission;
use crate::assist::settings::{self, Settings};
use crate::audit::{self, action};
use crate::workspace_db;

pub async fn get_settings(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<Settings>, ApiError> {
    let ws = parse(&ws)?;
    holder(&state, &current, ws, Permission::ManageLlm).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = settings::get(&mut tx).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

pub async fn put_settings(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    JsonBody(req): JsonBody<Settings>,
) -> Result<Json<Settings>, ApiError> {
    let ws = parse(&ws)?;
    holder(&state, &current, ws, Permission::ManageLlm).await?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let before = settings::get(&mut tx).await?;
    settings::set(&mut tx, req).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    if before != req {
        audit::record_full(
            &state.pool,
            Some(ws),
            Some(current.user_id),
            action::ASSIST_SETTINGS_CHANGED,
            &[],
            false,
            Some(&serde_json::json!({ "enabled": req.enabled })),
        )
        .await?;
    }
    Ok(Json(req))
}
