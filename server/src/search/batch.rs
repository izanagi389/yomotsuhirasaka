//! 関連記事の一括計算（08-search-and-related.md「計算のタイミング」、INV-REL-02、M3）。
//!
//! - 毎日、設定の時刻（`[search] related_batch_hour_utc`、UTC）を過ぎたら、ワークスペースごとに1回だけジョブを積む
//!   （[`spawn_scheduler`]）。サーバーが止まっていて時刻を逃した日の分は、起動した後に1回だけ積む。
//!   サーバーが複数台でも、`cms.related_batches` の行を書き換えた1台だけが積む。
//! - 管理APIの「今すぐ再計算」も、同じジョブを積む（毎日の分の記録は書き換えない）。同じワークスペースのジョブが待っている・
//!   実行中なら、新しく積まない。
//! - ジョブは、記事のベクトルがあるすべての記事の関連記事を計算し直し、返す一覧（`related::listed`）が変わった記事を
//!   Webhook（`entries.related_changed`）で知らせる。保存済みのベクトルを使うので、LLMの費用はかからない。
//!   途中で失敗してやり直したときは、前の試行で変わった記事は知らせない（便利のための通知で、一覧は正しく保たれる）。

use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use super::{related, switch};
use crate::config::Config;
use crate::jobs::{self, Outcome};
use crate::keys::KeyRing;
use crate::{webhooks, workspace_db};

pub const JOB_KIND: &str = "related_batch";
const MAX_ATTEMPTS: i32 = 5;
/// 毎日の一括計算を積むかを見る間隔。
const CHECK_INTERVAL: Duration = Duration::from_secs(300);

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Job {
    workspace_id: Uuid,
}

/// ワークスペースの一括計算のジョブを積む。同じワークスペースのジョブが待っている・実行中なら積まずに false。
pub async fn enqueue<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    keys: &KeyRing,
    workspace_id: Uuid,
) -> Result<bool> {
    jobs::enqueue_unless_queued(
        executor,
        keys,
        Some(workspace_id),
        JOB_KIND,
        &serde_json::to_vec(&Job { workspace_id })?,
        MAX_ATTEMPTS,
        &format!("{JOB_KIND}:{workspace_id}"),
    )
    .await
}

/// 今日の分（直近の予定の時刻の日）をまだ積んでいないワークスペースに、一括計算のジョブを積む。積んだワークスペースのIDを返す。
pub async fn schedule_due(pool: &PgPool, keys: &KeyRing, hour_utc: u8) -> Result<Vec<Uuid>> {
    let mut tx = pool.begin().await?;
    // 直近の予定の日 = (今 - 時刻) の日付（UTC）。行を書き換えた（または足した）ワークスペースだけが返る
    let due: Vec<Uuid> = sqlx::query_scalar(
        "WITH today AS (
           SELECT ((now() AT TIME ZONE 'UTC') - make_interval(hours => $1))::date AS d
         )
         INSERT INTO cms.related_batches (workspace_id, last_run_on)
         SELECT w.id, today.d FROM cms.workspaces w, today WHERE w.schema_version IS NOT NULL
         ON CONFLICT (workspace_id) DO UPDATE SET last_run_on = EXCLUDED.last_run_on
           WHERE cms.related_batches.last_run_on < EXCLUDED.last_run_on
         RETURNING workspace_id",
    )
    .bind(i32::from(hour_utc))
    .fetch_all(&mut *tx)
    .await?;
    for ws in &due {
        enqueue(&mut *tx, keys, *ws).await?;
    }
    tx.commit().await?;
    Ok(due)
}

/// 毎日の一括計算を積む常駐の処理。
pub fn spawn_scheduler(state: crate::http::AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let hour_utc = state.config.search.related_batch_hour_utc;
        let mut ticker = tokio::time::interval(CHECK_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if let Err(e) = schedule_due(&state.pool, &state.keys, hour_utc).await {
                tracing::error!("related entries batch scheduler: {e:#}");
            }
        }
    })
}

/// ジョブの本体。
pub async fn run(
    pool: &PgPool,
    keys: &KeyRing,
    config: &Config,
    payload: &[u8],
) -> Result<Outcome> {
    let Ok(job) = serde_json::from_slice::<Job>(payload) else {
        return Ok(Outcome::Fail("invalid_payload"));
    };
    let ws = job.workspace_id;
    let changed = run_workspace(pool, config, ws).await?;
    if changed.is_empty() {
        return Ok(Outcome::Done);
    }
    let Some(db) = workspace_db::lookup(pool, ws).await? else {
        return Ok(Outcome::Done);
    };
    let mut refs = Vec::with_capacity(changed.len());
    let mut tx = workspace_db::begin(pool, &db).await?;
    for id in changed {
        if let Some(r) = webhooks::entry_ref(&mut tx, id).await? {
            refs.push(r);
        }
    }
    tx.commit().await?;
    webhooks::emit_related_changed(pool, keys, ws, &refs).await?;
    Ok(Outcome::Done)
}

/// ワークスペースの関連記事を計算し直し、返す一覧が変わった記事のIDを返す。計算できないワークスペースは空。
pub async fn run_workspace(pool: &PgPool, config: &Config, ws: Uuid) -> Result<Vec<Uuid>> {
    let (Some(applied), Some(db)) = (
        crate::content::applied_schema(pool, ws).await?,
        workspace_db::lookup(pool, ws).await?,
    ) else {
        return Ok(Vec::new());
    };
    let mut tx = workspace_db::begin(pool, &db).await?;
    let serving = switch::serving(config, &mut tx).await?;
    tx.commit().await?;
    let (Some(s), Some(vs)) = (serving, super::vector_schema(pool).await?) else {
        return Ok(Vec::new());
    };
    related::recompute_all(
        pool,
        &db,
        &applied.manifest,
        &vs,
        &s.embedding_model,
        s.embedding_dimensions,
    )
    .await
}
