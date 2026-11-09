//! 管理API: 編集者向けのAI支援（10-llm-and-budget.md「編集者向けのAI支援」、Q69, Q70、M4）。
//!
//! | メソッド | パス | 内容 | 誰が |
//! |---|---|---|---|
//! | GET | /api/admin/workspaces/{ws}/assist-settings | 支援を使うか（`enabled`） | 重い権限 manageLlm を持つ人・サーバー管理者 |
//! | PUT | /api/admin/workspaces/{ws}/assist-settings | 同じく、変える | 同上（再認証） |
//! | POST | /api/admin/workspaces/{ws}/entries/{id}/assist/conflicts | いまの下書きについて、矛盾の検知を始める（公開の確認画面） | 記事を編集できる人 |
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/assist/conflicts | いまの下書きについての結果 | 同上 |
//! | POST | /api/admin/workspaces/{ws}/entries/{id}/assist/stale | いまの下書きについて、見直しの手がかりを作る（ボタン） | 同上 |
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/assist/stale | その結果 | 同上 |
//! | POST | /api/admin/workspaces/{ws}/question-log/candidates/{id}/outline | 候補から、書くべき記事の構成案を作る（ボタン） | 重い権限 viewQuestionLog を持つ人（再認証） |
//! | GET | /api/admin/workspaces/{ws}/question-log/candidates/{id}/outline | その結果 | 同上 |
//!
//! - 有効にすると、公開前の下書きが外部のLLMの提供会社に送られる（管理画面で注意を出す。Q177）。
//! - 変更は監査ログに残す（詳細は変えた後の値）。
//! - 矛盾の検知は非同期（`assist::conflicts`）。公開はこの結果を待たず、見もしない（INV-BUD-02）。
//! - 見直しの手がかりと構成案も非同期（`assist::tasks`）。構成案は質問文から作るので、質問ログの閲覧と同じく監査ログに残す。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use super::AppState;
use super::admin_entries::{db, editor, parse};
use super::admin_publishing::holder;
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::access::Permission;
use crate::assist::conflicts::{self, View};
use crate::assist::settings::{self, Settings};
use crate::assist::tasks::{self, TaskKind};
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
    if before != req {
        workspace_db::audit_full(
            &mut tx,
            &db,
            Some(ws),
            Some(current.user_id),
            action::ASSIST_SETTINGS_CHANGED,
            &[],
            false,
            Some(&serde_json::json!({ "enabled": req.enabled })),
        )
        .await?;
    }
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(req))
}

pub async fn start_conflicts(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<(StatusCode, Json<View>), ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    editor(&state, &current, ws).await?;
    let out = conflicts::start(&state, ws, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok((StatusCode::ACCEPTED, Json(out)))
}

pub async fn get_conflicts(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<View>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    editor(&state, &current, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = conflicts::view(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

pub async fn start_stale(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<(StatusCode, Json<tasks::View>), ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    editor(&state, &current, ws).await?;
    let out = tasks::start(&state, ws, TaskKind::Stale, &id.to_string())
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok((StatusCode::ACCEPTED, Json(out)))
}

pub async fn get_stale(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<tasks::View>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    editor(&state, &current, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = tasks::view(&mut tx, TaskKind::Stale, &id.to_string())
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

/// 構成案の操作の前の確認（重い権限 viewQuestionLog と再認証。候補のIDの形）。質問文を使うので監査ログに残す。
async fn outline_access(
    state: &AppState,
    current: &crate::auth::session::Current,
    ws: uuid::Uuid,
    id: &str,
) -> Result<(), ApiError> {
    holder(state, current, ws, Permission::ViewQuestionLog).await?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    // 候補のIDは、正規化した質問文のハッシュ（16進の64文字）
    if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ApiError::not_found());
    }
    audit::record(
        &state.pool,
        Some(ws),
        Some(current.user_id),
        action::QUESTION_LOG_VIEWED,
        &[],
    )
    .await?;
    Ok(())
}

pub async fn start_outline(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<(StatusCode, Json<tasks::View>), ApiError> {
    let ws = parse(&ws)?;
    outline_access(&state, &current, ws, &id).await?;
    let out = tasks::start(&state, ws, TaskKind::Outline, &id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok((StatusCode::ACCEPTED, Json(out)))
}

pub async fn get_outline(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<tasks::View>, ApiError> {
    let ws = parse(&ws)?;
    outline_access(&state, &current, ws, &id).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = tasks::view(&mut tx, TaskKind::Outline, &id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}
