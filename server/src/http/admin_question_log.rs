//! 管理API: 質問ログと「書くべき記事の候補」（09-qa-chatbot.md「質問ログ」、Q14, Q20, Q76、M4）。
//!
//! | メソッド | パス | 内容 | 誰が |
//! |---|---|---|---|
//! | GET | /api/admin/workspaces/{ws}/question-log/settings | 有効か・保存期間 | ワークスペースの管理者・サーバー管理者 |
//! | PUT | /api/admin/workspaces/{ws}/question-log/settings | 書いた項目だけを変える。無効にしたら、残っている質問文をすぐに消す | 同上（再認証） |
//! | GET | /api/admin/workspaces/{ws}/question-log | 候補（同じ質問をまとめたもの）と、直近の日ごとの件数 | 重い権限 viewQuestionLog を持つ人（再認証） |
//! | DELETE | /api/admin/workspaces/{ws}/question-log/candidates/{id} | 候補（同じ質問の記録すべて）を消す | 同上（再認証） |
//!
//! - 質問ログの閲覧は重い操作なので、再認証を求め、見るたびに監査ログに残す（11-permissions-and-audit.md、13-security.md）。
//! - 監査ログには、質問文を書かない（原則7）。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use super::AppState;
use super::admin_entries::{db, parse};
use super::admin_publishing::holder;
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::access::{self, Permission};
use crate::answer::log::{self, Settings, SettingsChange, View};
use crate::audit::{self, action};
use crate::workspace_db;

pub async fn get_settings(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<Settings>, ApiError> {
    let ws = parse(&ws)?;
    access::workspace_admin(&state.pool, &current, ws).await??;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = log::settings(&mut tx).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

pub async fn put_settings(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    JsonBody(change): JsonBody<SettingsChange>,
) -> Result<Json<Settings>, ApiError> {
    let ws = parse(&ws)?;
    access::workspace_admin(&state.pool, &current, ws).await??;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = match log::change_settings(&mut tx, &change).await? {
        Ok(out) => out,
        Err(problem) => {
            return Err(ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_question_log_settings",
                format!("The question log settings are not valid: {problem}"),
            ));
        }
    };
    if change.enabled.is_some() || change.retention_days.is_some() {
        workspace_db::audit_full(
            &mut tx,
            &db,
            Some(ws),
            Some(current.user_id),
            action::QUESTION_LOG_SETTINGS_CHANGED,
            &[],
            false,
            Some(
                &serde_json::json!({ "enabled": out.enabled, "retentionDays": out.retention_days }),
            ),
        )
        .await?;
    }
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

pub async fn view(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<View>, ApiError> {
    let ws = parse(&ws)?;
    holder(&state, &current, ws, Permission::ViewQuestionLog).await?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = log::view(&mut tx).await?;
    workspace_db::audit(
        &mut tx,
        &db,
        Some(ws),
        Some(current.user_id),
        action::QUESTION_LOG_VIEWED,
        &[],
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

pub async fn delete_candidate(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let ws = parse(&ws)?;
    holder(&state, &current, ws, Permission::ViewQuestionLog).await?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    // 候補のIDは、正規化した質問文のハッシュ（16進の64文字）
    if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ApiError::not_found());
    }
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let deleted = log::delete_candidate(&mut tx, &id).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    if !deleted {
        return Err(ApiError::not_found());
    }
    audit::record(
        &state.pool,
        Some(ws),
        Some(current.user_id),
        action::QUESTION_LOG_CANDIDATE_DELETED,
        &[],
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
