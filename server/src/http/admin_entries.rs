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

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
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
use crate::content::{Applied, applied_schema};
use crate::schema::manifest::{ContentType, FieldType};
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

async fn editor(state: &AppState, current: &Current, ws: Uuid) -> Result<Access, ApiError> {
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

/// 参照とスラッグの検査（ワークスペースのデータと照らし合わせるもの）。
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

pub async fn create(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    JsonBody(req): JsonBody<CreateEntry>,
) -> Result<(StatusCode, Json<Draft>), ApiError> {
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
    let columns = fields::convert_all(manifest, ct, &req.fields).map_err(invalid)?;
    let db = db(&state, ws).await?;
    let id = Uuid::new_v4();
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    check_against_data(&mut tx, ct, id, &locale, &columns).await??;
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
    tx.commit().await.map_err(anyhow::Error::from)?;
    if access.server_admin {
        audit::record(
            &state.pool,
            Some(ws),
            Some(current.user_id),
            audit::action::ENTRY_CREATED,
            &[id],
        )
        .await?;
    }
    Ok((StatusCode::CREATED, Json(draft)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveDraft {
    /// 保存の前に読んだリビジョン番号。
    revision: i64,
    /// 変えるフィールドだけ（null で空にする）。
    #[serde(default)]
    fields: Map<String, Value>,
}

pub(super) fn revision_conflict() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "revision_conflict",
        "The draft was changed by someone else. Reload it and apply your changes again.",
    )
}

pub async fn save_draft(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
    JsonBody(req): JsonBody<SaveDraft>,
) -> Result<Json<Draft>, ApiError> {
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
    let columns = fields::convert_all(manifest, ct, &req.fields).map_err(invalid)?;
    check_against_data(&mut tx, ct, id, &before.locale, &columns).await??;
    match entries::save(&mut tx, ct, id, req.revision, &columns).await? {
        Saved::Ok(_) => {}
        Saved::Conflict => return Err(revision_conflict()),
    }
    let draft = entries::draft(&mut tx, manifest, id)
        .await?
        .ok_or_else(ApiError::internal)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    if access.server_admin {
        audit::record(
            &state.pool,
            Some(ws),
            Some(current.user_id),
            audit::action::ENTRY_DRAFT_SAVED,
            &[id],
        )
        .await?;
    }
    Ok(Json(draft))
}
