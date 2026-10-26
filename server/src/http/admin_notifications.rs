//! 管理API: 管理者への通知（13-security.md「管理者への通知」、計画の15番）。
//!
//! | メソッド | パス | 内容 | 誰が |
//! |---|---|---|---|
//! | POST | /api/admin/workspaces/{ws}/notifications/{id}/read | ワークスペースの通知を「確認した」にする（一覧は「やること」） | そのワークスペースの管理者・サーバー管理者 |
//! | GET | /api/admin/notifications | インスタンス単位の通知（まだ確認していないもの） | サーバー管理者 |
//! | POST | /api/admin/notifications/{id}/read | インスタンス単位の通知を「確認した」にする | サーバー管理者 |
//!
//! - 「確認した」は1人ずつ。記録そのもの（監査ログ）は変えない（INV-AUD-02）。
//! - ワークスペースの管理者には、そのワークスペースの通知だけ。インスタンス単位の通知はサーバー管理者だけ（INV-AUD-08）。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use uuid::Uuid;

use super::AppState;
use super::error::ApiError;
use super::extract::AuthSession;
use crate::access;
use crate::audit::Entry;
use crate::notifications;

fn parse(id: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(id).map_err(|_| ApiError::not_found())
}

pub async fn read_workspace(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    access::workspace_admin(&state.pool, &current, ws).await??;
    if notifications::mark_read(&state.pool, Some(ws), current.user_id, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found())
    }
}

pub async fn instance(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
) -> Result<Json<Vec<Entry>>, ApiError> {
    access::server_admin(&current)?;
    Ok(Json(
        notifications::unread(&state.pool, None, current.user_id).await?,
    ))
}

pub async fn read_instance(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    access::server_admin(&current)?;
    if notifications::mark_read(&state.pool, None, current.user_id, parse(&id)?).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found())
    }
}
