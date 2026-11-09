//! 管理API: 翻訳。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | POST | /api/admin/workspaces/{ws}/entries/{id}/translations | 記事を複製して、別のロケールの翻訳を作る（`{locale}`） |
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/translation | 翻訳グループ、元の記事、反映済みの版、原文の更新 |
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/translation/source-diff | 元の記事の、反映済みの版から今の版までの差分 |
//! | POST | /api/admin/workspaces/{ws}/entries/{id}/translation/reflected | 「原文の更新を反映済み」（反映済みの版を元の記事の今の版に進める） |
//! | PUT | /api/admin/workspaces/{ws}/entries/{id}/translation/source | 元の記事を、同じ翻訳グループの別の記事に設定し直す（`{sourceEntryId}`） |
//!
//! - 作成は、記事の作成と同じく管理者・編集者・執筆者とサーバー管理者。移行待ちの間は受け付けない。
//! - 読むのは所属者全員とサーバー管理者。
//! - 「反映済み」は記事を編集できる人（執筆者以上）。重い権限もロックも要らない（内容を変えないため。Q164）。
//! - 元の記事の設定し直しは編集者以上（Q156）。どちらも監査ログに残す。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;

use super::AppState;
use super::admin_entries::{db, editor, invalid, parse, readable, reader, writable};
use super::admin_publishing::publisher;
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::audit;
use crate::content::entries::{self, Draft};
use crate::content::translations::{
    self, ReflectRefused, Refused, SetSourceRefused, SourceDiff, TranslationInfo,
};
use crate::workspace_db;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateTranslation {
    locale: String,
}

/// 同時に同じロケールの翻訳を作ったときの、一意の索引の違反か。
fn is_locale_conflict(e: &anyhow::Error) -> bool {
    e.downcast_ref::<sqlx::Error>()
        .and_then(|e| e.as_database_error())
        .is_some_and(|d| d.constraint() == Some("entries_group_locale"))
}

fn locale_exists() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "translation_exists",
        "This entry already has a translation in that locale.",
    )
}

pub async fn create(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
    JsonBody(req): JsonBody<CreateTranslation>,
) -> Result<(StatusCode, Json<Draft>), ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    let access = editor(&state, &current, ws).await?;
    let applied = writable(&state, ws).await?;
    if !applied.manifest.locales.contains(&req.locale) {
        return Err(invalid(vec![
            "locale: not a locale of this workspace".to_owned(),
        ]));
    }
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    if !entries::room_for(&mut tx, state.config.limits.max_entries_per_workspace, 1).await? {
        return Err(super::admin_entries::entry_limit_reached());
    }
    let draft =
        match translations::create(&mut tx, &applied.manifest, id, &req.locale, current.user_id)
            .await
        {
            Ok(Ok(d)) => d,
            Ok(Err(Refused::NotFound)) => return Err(ApiError::not_found()),
            Ok(Err(Refused::LocaleExists)) => return Err(locale_exists()),
            Ok(Err(Refused::Invalid(problems))) => return Err(invalid(problems)),
            Err(e) if is_locale_conflict(&e) => return Err(locale_exists()),
            Err(e) => return Err(e.into()),
        };
    if let Err(e) = tx.commit().await {
        let e = anyhow::Error::from(e);
        return Err(if is_locale_conflict(&e) {
            locale_exists()
        } else {
            e.into()
        });
    }
    if access.server_admin {
        audit::record(
            &state.pool,
            Some(ws),
            Some(current.user_id),
            audit::action::ENTRY_CREATED,
            &[draft.id],
        )
        .await?;
    }
    Ok((StatusCode::CREATED, Json(draft)))
}

pub async fn info(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<TranslationInfo>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    reader(&state, &current, ws).await?;
    readable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = translations::info(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

pub async fn source_diff(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<SourceDiff>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    reader(&state, &current, ws).await?;
    let applied = readable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = translations::source_diff(&mut tx, &applied.manifest, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

fn no_source() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "no_source",
        "This entry has no source entry.",
    )
}

pub async fn mark_reflected(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    editor(&state, &current, ws).await?;
    readable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    match translations::mark_reflected(&mut tx, id).await? {
        Ok(_) => {}
        Err(ReflectRefused::NotFound) => return Err(ApiError::not_found()),
        Err(ReflectRefused::NoSource) => return Err(no_source()),
    }
    workspace_db::audit(
        &mut tx,
        &db,
        Some(ws),
        Some(current.user_id),
        audit::action::TRANSLATION_SOURCE_REFLECTED,
        &[id],
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetSource {
    source_entry_id: Uuid,
}

pub async fn set_source(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
    JsonBody(req): JsonBody<SetSource>,
) -> Result<StatusCode, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    publisher(&state, &current, ws).await?;
    readable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    match translations::set_source(&mut tx, id, req.source_entry_id).await? {
        Ok(()) => {}
        Err(SetSourceRefused::NotFound) => return Err(ApiError::not_found()),
        Err(SetSourceRefused::InvalidSource) => {
            return Err(invalid(vec![
                "sourceEntryId: must be another entry in the same translation group".to_owned(),
            ]));
        }
    }
    workspace_db::audit(
        &mut tx,
        &db,
        Some(ws),
        Some(current.user_id),
        audit::action::TRANSLATION_SOURCE_CHANGED,
        &[id, req.source_entry_id],
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}
