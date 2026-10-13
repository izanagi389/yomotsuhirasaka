//! 管理API: 公開・非公開・削除、版、URLの変更履歴。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | POST | /api/admin/workspaces/{ws}/entries/{id}/publish | 公開する（`{revision, correction?}`） |
//! | POST | /api/admin/workspaces/{ws}/entries/{id}/unpublish | 非公開にする |
//! | DELETE | /api/admin/workspaces/{ws}/entries/{id} | 削除する |
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/versions | 版の一覧 |
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/versions/{vid} | 版（抹消した版は本文なし） |
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/url-history | 公開URLの変更履歴 |
//!
//! - 公開・訂正としての公開・非公開・削除は、管理者・編集者とサーバー管理者だけ（執筆者はできない。Q77, Q200、INV-PERM-01）。
//! - 公開は移行待ちの間は受け付けない（INV-SCH-04）。非公開と削除は、記事を急いで取り下げられるよう、移行待ちの間も受け付ける。
//! - 訂正としての公開は、誰が行っても監査ログに残す（11-permissions-and-audit.md）。それ以外は、所属していないサーバー管理者が
//!   行ったときに残す（Q122）。
//! - 版とURLの変更履歴を読むのは、下書きと同じく所属者全員とサーバー管理者。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use uuid::Uuid;

use super::AppState;
use super::admin_entries::{db, invalid, parse, readable, reader, revision_conflict, writable};
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::access::{self, Access, Role};
use crate::audit::{self, action};
use crate::auth::session::Current;
use crate::content::publishing::{self, Published, Refused, UrlChange, VersionDetail, VersionInfo};
use crate::workspace_db;

async fn publisher(state: &AppState, current: &Current, ws: Uuid) -> Result<Access, ApiError> {
    let access = reader(state, current, ws).await?;
    if !(access.server_admin || matches!(access.role, Some(Role::Admin | Role::Editor))) {
        return Err(access::Denied::Forbidden.into());
    }
    Ok(access)
}

fn refused(r: Refused) -> ApiError {
    match r {
        Refused::NotFound => ApiError::not_found(),
        Refused::RevisionConflict => revision_conflict(),
        Refused::Invalid(problems) => invalid(problems),
        Refused::AltTextMissing(places) => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "alt_text_missing",
            format!(
                "Every image needs alternative text or the decorative mark before publishing: {}",
                places.join("; ")
            ),
        ),
        Refused::UrlTaken => url_taken(),
        Refused::NothingToCorrect => ApiError::new(
            StatusCode::CONFLICT,
            "nothing_to_correct",
            "This entry has never been published, so it cannot be published as a correction.",
        ),
    }
}

fn url_taken() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "url_taken",
        "Another published entry already uses this URL.",
    )
}

/// 公開URLの一意の索引の違反か（同時に同じURLで公開したとき）。
fn is_url_conflict(e: &anyhow::Error) -> bool {
    e.downcast_ref::<sqlx::Error>()
        .and_then(|e| e.as_database_error())
        .is_some_and(|d| d.constraint() == Some("entries_published_url"))
}

async fn audit_if(
    state: &AppState,
    always: bool,
    access: &Access,
    current: &Current,
    ws: Uuid,
    act: &str,
    id: Uuid,
) -> Result<(), ApiError> {
    if always || access.server_admin {
        audit::record(&state.pool, Some(ws), Some(current.user_id), act, &[id]).await?;
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Publish {
    /// 公開する下書きのリビジョン番号（読んだときの値）。
    revision: i64,
    /// 訂正として公開するか。
    #[serde(default)]
    correction: bool,
}

pub async fn publish(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
    JsonBody(req): JsonBody<Publish>,
) -> Result<Json<Published>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    let access = publisher(&state, &current, ws).await?;
    let applied = writable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let result = publishing::publish(
        &mut tx,
        &applied.manifest,
        applied.version,
        id,
        req.revision,
        req.correction,
        current.user_id,
    )
    .await;
    let published = match result {
        Ok(Ok(p)) => p,
        Ok(Err(r)) => return Err(refused(r)),
        Err(e) if is_url_conflict(&e) => return Err(url_taken()),
        Err(e) => return Err(e.into()),
    };
    match tx.commit().await {
        Ok(()) => {}
        Err(e) => {
            let e = anyhow::Error::from(e);
            return Err(if is_url_conflict(&e) {
                url_taken()
            } else {
                e.into()
            });
        }
    }
    let act = if req.correction {
        action::ENTRY_PUBLISHED_AS_CORRECTION
    } else {
        action::ENTRY_PUBLISHED
    };
    audit_if(&state, req.correction, &access, &current, ws, act, id).await?;
    Ok(Json(published))
}

pub async fn unpublish(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    let access = publisher(&state, &current, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    publishing::unpublish(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    audit_if(
        &state,
        false,
        &access,
        &current,
        ws,
        action::ENTRY_UNPUBLISHED,
        id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    let access = publisher(&state, &current, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    if !publishing::delete(&mut tx, id).await? {
        return Err(ApiError::not_found());
    }
    tx.commit().await.map_err(anyhow::Error::from)?;
    audit_if(
        &state,
        false,
        &access,
        &current,
        ws,
        action::ENTRY_DELETED,
        id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn versions(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<Vec<VersionInfo>>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    reader(&state, &current, ws).await?;
    readable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = publishing::versions(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

pub async fn version(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id, vid)): Path<(String, String, String)>,
) -> Result<Json<VersionDetail>, ApiError> {
    let (ws, id, vid) = (parse(&ws)?, parse(&id)?, parse(&vid)?);
    reader(&state, &current, ws).await?;
    readable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = publishing::version(&mut tx, id, vid)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

pub async fn url_history(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<Vec<UrlChange>>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    reader(&state, &current, ws).await?;
    readable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = publishing::url_history(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}
