//! 管理API: 訂正の説明文と定型文、鮮度（確認済みの記録・確認期限の上書き）、未記入の訂正と確認期限切れの一覧
//! （06-corrections-and-freshness.md）。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | PUT | /api/admin/workspaces/{ws}/entries/{id}/versions/{vid}/correction-note | 訂正の説明を書く・書き直す（`{text}`） |
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/versions/{vid}/correction-note/history | 説明の編集の履歴 |
//! | GET | /api/admin/workspaces/{ws}/correction-templates | 定型文 |
//! | PUT | /api/admin/workspaces/{ws}/correction-templates | 定型文を置き換える（管理者） |
//! | GET | /api/admin/workspaces/{ws}/corrections/unwritten | 説明を書いていない訂正 |
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/freshness | 鮮度と確認済みの記録の履歴 |
//! | POST | /api/admin/workspaces/{ws}/entries/{id}/verify | 「確認済み」を記録する（`{memo}`） |
//! | PUT | /api/admin/workspaces/{ws}/entries/{id}/review | 確認期限の上書き（`{intervalDays?, disabled}`） |
//! | GET | /api/admin/workspaces/{ws}/reviews/expired | 確認期限切れの公開中の記事 |
//!
//! - 説明の記入と確認済みの記録は、重い権限「訂正としての公開と確認済みの記録」を持つ人（管理者・編集者の既定と、
//!   個人への付与。Q76、INV-PERM-03）。期限の上書きは、公開できる人（管理者・編集者とサーバー管理者）。読むのは所属者全員。
//! - 定型文を変えられるのは管理者とサーバー管理者。
//! - 確認済みの記録と、説明の記入・書き直しは、監査ログに残す（IDと操作の種類だけ）。
//! - 「確認済み」は記事ごとの操作だけで、一括の操作は作らない（Q46）。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;

use super::AppState;
use super::admin_entries::{db, invalid, parse, readable, reader};
use super::admin_publishing::{holder, publisher};
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::access::{self, Permission};
use crate::audit::action;
use crate::content::corrections::{self, NoteEdit, Template, Unwritten};
use crate::content::freshness::{self, Expired, FreshnessDetail, ReviewOverride};
use crate::workspace_db;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NoteBody {
    /// 説明（空なら「書いていない」に戻す）。
    text: String,
}

pub async fn put_note(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id, vid)): Path<(String, String, String)>,
    JsonBody(req): JsonBody<NoteBody>,
) -> Result<StatusCode, ApiError> {
    let (ws, id, vid) = (parse(&ws)?, parse(&id)?, parse(&vid)?);
    holder(&state, &current, ws, Permission::Corrections).await?;
    let text = req.text.trim();
    if !corrections::valid_note(text) {
        return Err(invalid(vec![format!(
            "text: at most {} characters without control characters",
            corrections::MAX_NOTE_CHARS
        )]));
    }
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    if !corrections::save_note(&mut tx, id, vid, text, Some(current.user_id)).await? {
        return Err(ApiError::not_found());
    }
    workspace_db::audit(
        &mut tx,
        &db,
        Some(ws),
        Some(current.user_id),
        action::CORRECTION_NOTE_EDITED,
        &[id, vid],
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn note_history(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id, vid)): Path<(String, String, String)>,
) -> Result<Json<Vec<NoteEdit>>, ApiError> {
    let (ws, id, vid) = (parse(&ws)?, parse(&id)?, parse(&vid)?);
    reader(&state, &current, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = corrections::note_history(&mut tx, id, vid).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

pub async fn get_templates(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<Vec<Template>>, ApiError> {
    let ws = parse(&ws)?;
    reader(&state, &current, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = corrections::templates(&mut tx).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

pub async fn put_templates(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    JsonBody(req): JsonBody<Vec<Template>>,
) -> Result<Json<Vec<Template>>, ApiError> {
    let ws = parse(&ws)?;
    access::workspace_admin(&state.pool, &current, ws).await??;
    let applied = readable(&state, ws).await?;
    let problems = corrections::template_problems(&applied.manifest, &req);
    if !problems.is_empty() {
        return Err(invalid(problems));
    }
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    corrections::replace_templates(&mut tx, &req).await?;
    let out = corrections::templates(&mut tx).await?;
    workspace_db::audit(
        &mut tx,
        &db,
        Some(ws),
        Some(current.user_id),
        action::CORRECTION_TEMPLATES_CHANGED,
        &[],
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

pub async fn unwritten(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<Vec<Unwritten>>, ApiError> {
    let ws = parse(&ws)?;
    reader(&state, &current, ws).await?;
    let applied = readable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = corrections::unwritten(&mut tx, &applied.manifest).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

pub async fn get_freshness(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<FreshnessDetail>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    reader(&state, &current, ws).await?;
    let applied = readable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = freshness::detail(&mut tx, &applied.manifest, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifyBody {
    #[serde(default)]
    memo: String,
}

pub async fn verify(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
    JsonBody(req): JsonBody<VerifyBody>,
) -> Result<StatusCode, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    holder(&state, &current, ws, Permission::Corrections).await?;
    let memo = req.memo.trim();
    if memo.chars().count() > freshness::MAX_MEMO_CHARS
        || memo.chars().any(|c| c.is_control() && c != '\n')
    {
        return Err(invalid(vec![format!(
            "memo: at most {} characters without control characters",
            freshness::MAX_MEMO_CHARS
        )]));
    }
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    if !freshness::verify(&mut tx, id, current.user_id, memo).await? {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "not_published",
            "Only entries that have been published can be marked as verified.",
        ));
    }
    workspace_db::audit(
        &mut tx,
        &db,
        Some(ws),
        Some(current.user_id),
        action::ENTRY_VERIFIED,
        &[id],
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn put_review(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
    JsonBody(req): JsonBody<ReviewOverride>,
) -> Result<StatusCode, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    publisher(&state, &current, ws).await?;
    if req
        .interval_days
        .is_some_and(|d| !(1..=freshness::MAX_INTERVAL_DAYS).contains(&d))
    {
        return Err(invalid(vec![format!(
            "intervalDays: must be 1 to {}",
            freshness::MAX_INTERVAL_DAYS
        )]));
    }
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    if !freshness::set_review(&mut tx, id, req).await? {
        return Err(ApiError::not_found());
    }
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn expired(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<Vec<Expired>>, ApiError> {
    let ws = parse(&ws)?;
    reader(&state, &current, ws).await?;
    let applied = readable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = freshness::expired(&mut tx, &applied.manifest).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}
