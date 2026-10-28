//! 管理API: 関連記事の固定と除外（08-search-and-related.md「関連記事」、Q60・Q157、M3）。
//!
//! | メソッド | パス | 内容 | 誰が |
//! |---|---|---|---|
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/related | 固定（順番付き）と除外の翻訳グループ | 所属者・サーバー管理者 |
//! | PUT | /api/admin/workspaces/{ws}/entries/{id}/related | 固定と除外を置き換える。その記事の関連記事を、その場で計算し直す | 公開できる人（編集者・管理者）・サーバー管理者 |
//!
//! - 固定・除外は版ではなく記事に持ち、変えたらすぐに効く。指すのは翻訳グループで、返すときに同じロケールの記事へ解決する。
//! - 検査: 固定は [`MAX_PINNED`] 件、除外は [`MAX_EXCLUDED`] 件まで。同じワークスペースの、同じコンテンツタイプの翻訳グループで、
//!   自分の翻訳グループは指せない。固定と除外の両方には入れられない。指した記事が後で非公開・削除になっても設定は残し、返すときに落とす。
//! - 監査ログは、公開の操作と同じく、サーバー管理者が操作したときに残す。

use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AppState;
use super::admin_entries::{db, parse, reader};
use super::admin_publishing::publisher;
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::audit::{self, action};
use crate::workspace_db;

/// 固定と除外の数の上限。
pub const MAX_PINNED: usize = 5;
pub const MAX_EXCLUDED: usize = 50;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelatedSettings {
    pinned: Vec<Uuid>,
    excluded: Vec<Uuid>,
}

async fn read_settings(
    conn: &mut sqlx::PgConnection,
    entry_id: Uuid,
) -> Result<RelatedSettings, ApiError> {
    let (pinned, excluded): (Vec<Uuid>, Vec<Uuid>) =
        sqlx::query_as("SELECT pinned, excluded FROM related_settings WHERE entry_id = $1")
            .bind(entry_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(anyhow::Error::from)?
            .unwrap_or_default();
    Ok(RelatedSettings { pinned, excluded })
}

/// 記事のコンテンツタイプと翻訳グループ（削除済みなら None）。
async fn entry_of(
    conn: &mut sqlx::PgConnection,
    entry_id: Uuid,
) -> Result<Option<(String, Uuid)>, ApiError> {
    Ok(sqlx::query_as(
        "SELECT content_type, translation_group_id FROM entries WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(entry_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(anyhow::Error::from)?)
}

pub async fn get(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<RelatedSettings>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    reader(&state, &current, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    if entry_of(&mut tx, id).await?.is_none() {
        return Err(ApiError::not_found());
    }
    let settings = read_settings(&mut tx, id).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(settings))
}

/// 重複のない並びか。
fn distinct(ids: &[Uuid]) -> bool {
    let mut sorted = ids.to_vec();
    sorted.sort();
    sorted.windows(2).all(|w| w[0] != w[1])
}

pub async fn put(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
    JsonBody(req): JsonBody<RelatedSettings>,
) -> Result<Json<RelatedSettings>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    let access = publisher(&state, &current, ws).await?;
    if req.pinned.len() > MAX_PINNED || req.excluded.len() > MAX_EXCLUDED {
        return Err(ApiError::bad_request(format!(
            "`pinned` takes at most {MAX_PINNED} and `excluded` at most {MAX_EXCLUDED} translation groups."
        )));
    }
    if !distinct(&req.pinned)
        || !distinct(&req.excluded)
        || req.pinned.iter().any(|g| req.excluded.contains(g))
    {
        return Err(ApiError::bad_request(
            "A translation group may appear only once in `pinned` and `excluded` together.",
        ));
    }
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let Some((content_type, group)) = entry_of(&mut tx, id).await? else {
        return Err(ApiError::not_found());
    };
    let all: Vec<Uuid> = [req.pinned.as_slice(), req.excluded.as_slice()].concat();
    if all.contains(&group) {
        return Err(ApiError::bad_request(
            "An entry cannot pin or exclude its own translation group.",
        ));
    }
    // 指す翻訳グループが、このワークスペースの同じコンテンツタイプの（削除されていない）記事のものか
    let known: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT translation_group_id FROM entries
         WHERE translation_group_id = ANY($1) AND content_type = $2 AND deleted_at IS NULL",
    )
    .bind(&all)
    .bind(&content_type)
    .fetch_all(&mut *tx)
    .await
    .map_err(anyhow::Error::from)?;
    if all.iter().any(|g| !known.contains(g)) {
        return Err(ApiError::bad_request(
            "Every translation group must belong to an entry of the same content type in this workspace.",
        ));
    }
    sqlx::query(
        "INSERT INTO related_settings (entry_id, pinned, excluded, updated_by, updated_at)
         VALUES ($1, $2, $3, $4, now())
         ON CONFLICT (entry_id) DO UPDATE SET pinned = $2, excluded = $3, updated_by = $4, updated_at = now()",
    )
    .bind(id)
    .bind(&req.pinned)
    .bind(&req.excluded)
    .bind(current.user_id)
    .execute(&mut *tx)
    .await
    .map_err(anyhow::Error::from)?;
    // その記事の関連記事を、その場で計算し直す（固定・除外は返すときにも照合するので、計算できなくても効く）
    if state.config.llm.is_some() {
        let serving = crate::search::switch::serving(&state.config, &mut tx).await?;
        let vs = crate::search::vector_schema(&mut *tx).await?;
        let applied = crate::content::applied_schema(&state.pool, ws).await?;
        if let (Some(s), Some(vs), Some(applied)) = (serving, vs, applied) {
            crate::search::related::compute(
                &mut tx,
                &applied.manifest,
                &vs,
                &s.embedding_model,
                s.embedding_dimensions,
                id,
            )
            .await?;
        }
    }
    let settings = read_settings(&mut tx, id).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    if access.server_admin {
        audit::record(
            &state.pool,
            Some(ws),
            Some(current.user_id),
            action::ENTRY_RELATED_CHANGED,
            &[id],
        )
        .await?;
    }
    Ok(Json(settings))
}
