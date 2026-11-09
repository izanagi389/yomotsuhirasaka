//! 管理API: 関連記事の固定と除外（08-search-and-related.md「関連記事」、Q60・Q157、M3）。
//!
//! | メソッド | パス | 内容 | 誰が |
//! |---|---|---|---|
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/related | 固定（順番付き）と除外の翻訳グループと、今返している一覧 | 所属者・サーバー管理者 |
//! | PUT | /api/admin/workspaces/{ws}/entries/{id}/related | 固定と除外を置き換える。その記事の関連記事を、その場で計算し直す | 公開できる人（編集者・管理者）・サーバー管理者 |
//! | POST | /api/admin/workspaces/{ws}/related/recompute | ワークスペースの関連記事を、今すぐ一括で計算し直す（ジョブを積んで 202） | 管理者・サーバー管理者 |
//!
//! - 固定・除外は版ではなく記事に持ち、変えたらすぐに効く。指すのは翻訳グループで、返すときに同じロケールの記事へ解決する。
//! - 検査: 固定は [`MAX_PINNED`] 件、除外は [`MAX_EXCLUDED`] 件まで。同じワークスペースの、同じコンテンツタイプの翻訳グループで、
//!   自分の翻訳グループは指せない。固定と除外の両方には入れられない。指した記事が後で非公開・削除になっても設定は残し、返すときに落とす。
//! - 固定・除外を変えて、返す一覧（`related::listed`）が変わったら、その記事を Webhook（`entries.related_changed`）で知らせる
//!   （INV-REL-02）。一括再計算で変わった記事も、同じイベントで知らせる（`search::batch`）。
//! - 一括再計算は、保存済みのベクトルを使うのでLLMの費用はかからない。同じワークスペースの一括計算が待っている・実行中なら、
//!   新しく積まない。
//! - 監査ログは、公開の操作と同じく、サーバー管理者が操作したときに残す。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AppState;
use super::admin_entries::{db, parse, reader};
use super::admin_publishing::publisher;
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::access::Role;
use crate::audit::{self, action};
use crate::search::related;
use crate::{webhooks, workspace_db};

/// 固定と除外の数の上限。
pub const MAX_PINNED: usize = 5;
pub const MAX_EXCLUDED: usize = 50;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelatedSettings {
    pinned: Vec<Uuid>,
    excluded: Vec<Uuid>,
}

/// 一覧で見せる1件（今返している関連記事。配信と同じ照合をかけた上限 10 件）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Current {
    entry_id: Uuid,
    pinned: bool,
    score: f64,
}

/// GET・PUT の応答: 固定と除外と、今返している一覧（固定・除外を決める材料に見せる）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelatedView {
    pinned: Vec<Uuid>,
    excluded: Vec<Uuid>,
    current: Vec<Current>,
}

async fn view(conn: &mut sqlx::PgConnection, entry_id: Uuid) -> Result<RelatedView, ApiError> {
    let RelatedSettings { pinned, excluded } = read_settings(conn, entry_id).await?;
    let current = related::pick(conn, entry_id, related::MAX_FIRST)
        .await?
        .into_iter()
        .map(|p| Current {
            entry_id: p.entry_id,
            pinned: p.pinned,
            score: p.score,
        })
        .collect();
    Ok(RelatedView {
        pinned,
        excluded,
        current,
    })
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
) -> Result<Json<RelatedView>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    reader(&state, &current, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    if entry_of(&mut tx, id).await?.is_none() {
        return Err(ApiError::not_found());
    }
    let settings = view(&mut tx, id).await?;
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
) -> Result<Json<RelatedView>, ApiError> {
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
    let before = related::listed(&mut tx, id).await?;
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
        let setup =
            crate::workspace_llm::embedding(&state.pool, &state.keys, &state.config, ws).await?;
        let serving =
            crate::search::switch::serving(setup.as_ref().map(|s| &s.llm), &mut tx).await?;
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
    let settings = view(&mut tx, id).await?;
    let changed = settings
        .current
        .iter()
        .map(|c| c.entry_id)
        .collect::<Vec<_>>()
        != before;
    let entry = webhooks::entry_ref(&mut tx, id).await?;
    if access.server_admin {
        workspace_db::audit(
            &mut tx,
            &db,
            Some(ws),
            Some(current.user_id),
            action::ENTRY_RELATED_CHANGED,
            &[id],
        )
        .await?;
    }
    tx.commit().await.map_err(anyhow::Error::from)?;
    if changed && let Some(entry) = entry {
        webhooks::emit_related_changed(&state.pool, &state.keys, ws, &[entry]).await?;
    }
    Ok(Json(settings))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recompute {
    /// 新しくジョブを積んだか（同じワークスペースの一括計算が待っている・実行中なら false）。
    queued: bool,
}

pub async fn recompute(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<(StatusCode, Json<Recompute>), ApiError> {
    let ws = parse(&ws)?;
    let access = reader(&state, &current, ws).await?;
    if !(access.server_admin || access.role == Some(Role::Admin)) {
        return Err(crate::access::Denied::Forbidden.into());
    }
    let queued = crate::search::batch::enqueue(&state.pool, &state.keys, ws).await?;
    if access.server_admin {
        audit::record(
            &state.pool,
            Some(ws),
            Some(current.user_id),
            action::RELATED_RECOMPUTE_STARTED,
            &[],
        )
        .await?;
    }
    Ok((StatusCode::ACCEPTED, Json(Recompute { queued })))
}
