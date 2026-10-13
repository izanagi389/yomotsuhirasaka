//! 管理API: 翻訳。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | POST | /api/admin/workspaces/{ws}/entries/{id}/translations | 記事を複製して、別のロケールの翻訳を作る（`{locale}`） |
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/translation | 翻訳グループ、元の記事、反映済みの版 |
//!
//! - 作成は、記事の作成と同じく管理者・編集者・執筆者とサーバー管理者。移行待ちの間は受け付けない。
//! - 読むのは所属者全員とサーバー管理者。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;

use super::AppState;
use super::admin_entries::{db, editor, invalid, parse, readable, reader, writable};
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::audit;
use crate::content::entries::Draft;
use crate::content::translations::{self, Refused, TranslationInfo};
use crate::workspace_db;

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
