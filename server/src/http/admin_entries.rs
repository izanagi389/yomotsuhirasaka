//! 管理API: 記事と下書き。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | GET | /api/admin/workspaces/{ws}/entries | 記事の一覧 |
//! | POST | /api/admin/workspaces/{ws}/entries | 記事を作る（下書き） |
//! | GET | /api/admin/workspaces/{ws}/entries/{id} | 記事の下書き |
//! | PUT | /api/admin/workspaces/{ws}/entries/{id}/draft | 下書きを保存する（リビジョン番号の照合つき） |
//!
//! - 読むのは所属者全員（閲覧者も下書きを見られる。Q192）とサーバー管理者。
//! - 作成と保存は管理者・編集者・執筆者とサーバー管理者（執筆者は他の人の記事も編集できる。Q192）。
//!   サーバー管理者が所属せずに書いたときは、そのワークスペースの監査ログに残す（Q122）。
//! - 移行待ちのワークスペースでは、作成と保存を受け付けない（INV-SCH-04）。
//! - 下書きの保存は、送られたリビジョン番号が最新でなければ 409 にし、何も書かない（INV-EDIT-01、Q185）。
//! - 作成と保存では、保存前のフック（beforeSave）の直しを重ねてから検査して保存する。フックが失敗したら直さずに保存し、
//!   応答の `hookFailure` で知らせる（`content/hooks.rs`）。保存の後、保存後のフック（afterSave）を積む。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::AppState;
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::access::{self, Access, Role};
use crate::audit;
use crate::auth::session::Current;
use crate::content::entries::{self, Draft, Saved, Summary};
use crate::content::fields::{self, Column};
use crate::content::hooks;
use crate::content::{Applied, applied_schema};
use crate::schema::manifest::{ContentType, FieldType};
use crate::script::{Failure, Host};
use crate::workspace_db;

pub(super) fn parse(id: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(id).map_err(|_| ApiError::not_found())
}

fn can_edit(access: &Access) -> bool {
    access.server_admin || matches!(access.role, Some(Role::Admin | Role::Editor | Role::Author))
}

pub(super) fn not_deployed() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "schema_not_deployed",
        "No template has been deployed to this workspace yet.",
    )
}

pub(super) fn invalid(problems: Vec<String>) -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_entry",
        format!("The entry is not valid: {}", problems.join("; ")),
    )
}

/// 読むためのスキーマ（移行待ちでも、適用済みの版で読める）。
pub(super) async fn readable(state: &AppState, ws: Uuid) -> Result<Applied, ApiError> {
    applied_schema(&state.pool, ws)
        .await?
        .ok_or_else(not_deployed)
}

/// 書くためのスキーマ（移行待ちなら断る）。
pub(super) async fn writable(state: &AppState, ws: Uuid) -> Result<Applied, ApiError> {
    let applied = readable(state, ws).await?;
    if applied.pending {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "schema_migration_pending",
            "This workspace is waiting for a schema migration. Editing is paused until it completes.",
        ));
    }
    Ok(applied)
}

pub(super) async fn reader(
    state: &AppState,
    current: &Current,
    ws: Uuid,
) -> Result<Access, ApiError> {
    Ok(access::member(&state.pool, current, ws).await??)
}

pub(super) async fn editor(
    state: &AppState,
    current: &Current,
    ws: Uuid,
) -> Result<Access, ApiError> {
    let access = reader(state, current, ws).await?;
    if !can_edit(&access) {
        return Err(access::Denied::Forbidden.into());
    }
    Ok(access)
}

pub(super) async fn db(state: &AppState, ws: Uuid) -> Result<workspace_db::WorkspaceDb, ApiError> {
    workspace_db::lookup(&state.pool, ws)
        .await?
        .ok_or_else(ApiError::not_found)
}

/// 参照・スラッグ・画像のフィールドの検査（ワークスペースのデータと照らし合わせるもの）。
async fn check_against_data(
    conn: &mut sqlx::PgConnection,
    ct: &ContentType,
    entry_id: Uuid,
    locale: &str,
    columns: &[(String, Column)],
) -> Result<Result<(), ApiError>, ApiError> {
    for (name, value) in columns {
        let Some(field) = ct.field(name) else {
            continue;
        };
        match (field.kind, value) {
            (FieldType::Slug, Column::Text(Some(slug))) => {
                if entries::slug_taken(conn, ct, locale, slug, entry_id).await? {
                    return Ok(Err(ApiError::new(
                        StatusCode::CONFLICT,
                        "slug_taken",
                        format!(
                            "Another entry in this locale already uses this value of `{name}`."
                        ),
                    )));
                }
            }
            (FieldType::Reference, Column::Uuid(Some(id))) => {
                let to = field.reference_to.as_deref().unwrap_or_default();
                if !entries::groups_exist(conn, to, &[*id]).await? {
                    return Ok(Err(invalid(vec![format!("fields.{name}: unknown entry")])));
                }
            }
            (FieldType::Reference, Column::Uuids(Some(ids))) => {
                let to = field.reference_to.as_deref().unwrap_or_default();
                if !entries::groups_exist(conn, to, ids).await? {
                    return Ok(Err(invalid(vec![format!("fields.{name}: unknown entry")])));
                }
            }
            // 画像のフィールド: このワークスペースにある、削除されていないメディアだけ（INV-IMG-03）
            (FieldType::Image, Column::Json(Some(json))) => {
                let id = serde_json::from_str::<serde_json::Value>(json)
                    .ok()
                    .as_ref()
                    .and_then(crate::content::fields::image_media_id);
                let found = match id {
                    Some(id) => crate::media::store::exists(conn, id).await?,
                    None => false,
                };
                if !found {
                    return Ok(Err(invalid(vec![format!("fields.{name}: unknown media")])));
                }
            }
            _ => {}
        }
    }
    Ok(Ok(()))
}

pub async fn list(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<Vec<Summary>>, ApiError> {
    let ws = parse(&ws)?;
    reader(&state, &current, ws).await?;
    let applied = readable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = entries::list(&mut tx, &applied.manifest).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

pub async fn get(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<Draft>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    reader(&state, &current, ws).await?;
    let applied = readable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let draft = entries::draft(&mut tx, &applied.manifest, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(draft))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateEntry {
    content_type: String,
    /// 省略時はテンプレートの既定のロケール。
    locale: Option<String>,
    #[serde(default)]
    fields: Map<String, Value>,
}

/// 保存した下書き（保存前のフックが失敗したときは、その種類を添える）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedDraft {
    #[serde(flatten)]
    draft: Draft,
    #[serde(skip_serializing_if = "Option::is_none")]
    hook_failure: Option<&'static str>,
}

/// 記事の数の上限に達している。
pub(super) fn entry_limit_reached() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "entry_limit_reached",
        "This workspace has reached its entry limit.",
    )
}

pub async fn create(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    JsonBody(req): JsonBody<CreateEntry>,
) -> Result<(StatusCode, Json<SavedDraft>), ApiError> {
    let ws = parse(&ws)?;
    let access = editor(&state, &current, ws).await?;
    let applied = writable(&state, ws).await?;
    let manifest = &applied.manifest;
    let ct = manifest
        .content_type(&req.content_type)
        .ok_or_else(|| invalid(vec!["contentType: unknown content type".to_owned()]))?;
    let locale = req
        .locale
        .unwrap_or_else(|| manifest.default_locale.clone());
    if !manifest.locales.contains(&locale) {
        return Err(invalid(vec![
            "locale: not a locale of this workspace".to_owned(),
        ]));
    }
    // 送られた値そのものが正しくなければ、フックを動かさずに断る
    fields::convert_all(manifest, ct, &req.fields).map_err(invalid)?;
    let db = db(&state, ws).await?;
    let id = Uuid::new_v4();
    let entry = serde_json::json!({ "id": id, "contentType": ct.name, "locale": locale });
    let (changes, hook_failure) = hooks::before_save(
        &state.scripts,
        Host::new(state.pool.clone(), db.clone()),
        manifest,
        ct,
        entry.as_object().cloned().unwrap_or_default(),
        &Map::new(),
        req.fields,
    )
    .await;
    let columns = fields::convert_all(manifest, ct, &changes).map_err(invalid)?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    check_against_data(&mut tx, ct, id, &locale, &columns).await??;
    if !entries::room_for(&mut tx, state.config.limits.max_entries_per_workspace, 1).await? {
        return Err(entry_limit_reached());
    }
    entries::create(
        &mut tx,
        ct,
        id,
        &locale,
        Uuid::new_v4(),
        current.user_id,
        &columns,
    )
    .await?;
    let draft = entries::draft(&mut tx, manifest, id)
        .await?
        .ok_or_else(ApiError::internal)?;
    if access.server_admin {
        workspace_db::audit(
            &mut tx,
            &db,
            Some(ws),
            Some(current.user_id),
            audit::action::ENTRY_CREATED,
            &[id],
        )
        .await?;
    }
    tx.commit().await.map_err(anyhow::Error::from)?;
    schedule_after_save(&state, ct, ws, id, &locale).await;
    Ok((
        StatusCode::CREATED,
        Json(SavedDraft {
            draft,
            hook_failure: hook_failure.map(Failure::code),
        }),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveDraft {
    /// 保存の前に読んだリビジョン番号。
    revision: i64,
    /// 変えるフィールドだけ（null で空にする）。
    #[serde(default)]
    fields: Map<String, Value>,
    /// 編集のロック（記事の画面を開いたときに取ったもの）。APIから直接保存するときは付けない。
    #[serde(default)]
    lock_id: Option<Uuid>,
}

pub(super) fn revision_conflict() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "revision_conflict",
        "The draft was changed by someone else. Reload it and apply your changes again.",
    )
}

/// 保存後のフックを積む。積めなくても保存は確定しているので、記録して続ける。
async fn schedule_after_save(state: &AppState, ct: &ContentType, ws: Uuid, id: Uuid, locale: &str) {
    if let Err(e) = hooks::schedule_after_save(&state.pool, &state.keys, ct, ws, id, locale).await {
        tracing::error!(workspace_id = %ws, entry_id = %id, "cannot queue the afterSave hook: {e:#}");
    }
}

pub async fn save_draft(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
    JsonBody(req): JsonBody<SaveDraft>,
) -> Result<Json<SavedDraft>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    let access = editor(&state, &current, ws).await?;
    let applied = writable(&state, ws).await?;
    let manifest = &applied.manifest;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let before = entries::draft(&mut tx, manifest, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let ct = manifest
        .content_type(&before.content_type)
        .ok_or_else(ApiError::not_found)?;
    fields::convert_all(manifest, ct, &req.fields).map_err(invalid)?;
    let entry = serde_json::json!({ "id": id, "contentType": ct.name, "locale": before.locale });
    let (changes, hook_failure) = hooks::before_save(
        &state.scripts,
        Host::new(state.pool.clone(), db.clone()),
        manifest,
        ct,
        entry.as_object().cloned().unwrap_or_default(),
        &before.fields,
        req.fields,
    )
    .await;
    let columns = fields::convert_all(manifest, ct, &changes).map_err(invalid)?;
    check_against_data(&mut tx, ct, id, &before.locale, &columns).await??;
    // ほかの画面が編集中なら保存しない（INV-LOCK-01。保存ではロックを取らない）
    if crate::content::locks::blocking_save(&mut tx, id, req.lock_id)
        .await?
        .is_some()
    {
        return Err(super::admin_locks::entry_locked());
    }
    match entries::save(&mut tx, ct, id, req.revision, &columns).await? {
        Saved::Ok(_) => {}
        Saved::Conflict => return Err(revision_conflict()),
    }
    let draft = entries::draft(&mut tx, manifest, id)
        .await?
        .ok_or_else(ApiError::internal)?;
    if access.server_admin {
        workspace_db::audit(
            &mut tx,
            &db,
            Some(ws),
            Some(current.user_id),
            audit::action::ENTRY_DRAFT_SAVED,
            &[id],
        )
        .await?;
    }
    tx.commit().await.map_err(anyhow::Error::from)?;
    schedule_after_save(&state, ct, ws, id, &before.locale).await;
    Ok(Json(SavedDraft {
        draft,
        hook_failure: hook_failure.map(Failure::code),
    }))
}
