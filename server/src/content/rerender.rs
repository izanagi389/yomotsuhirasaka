//! 公開中の版の本文の変換結果の作り直し（Q58、07-ai-delivery.md）。
//!
//! - 変換結果（`versions.rendered`）は公開のときに作る。それより前に公開した版（変換器が入る前）や、
//!   変換器の版が変わった後の版は、変換結果がないか古いので、ジョブで作り直す。
//! - ジョブは、サーバーの起動のときと、テンプレートのデプロイの後に、ワークスペースごとに積む（同じものは1つにまとめる）。
//! - 作り直すのは公開中の版だけ。本文を抹消した版（fields が空）は作らない。変換は、いま適用済みのスキーマのカスタムノードで行う。
//! - 変換できなかった版はそのままにして、ジョブをやり直す（回数に上限がある）。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::PgPool;
use uuid::Uuid;

use super::applied_schema;
use super::publishing::{self, Scripts};
use crate::jobs::Outcome;
use crate::keys::KeyRing;
use crate::script::{CONVERTER_VERSION, Engine, Host};
use crate::workspace_db;

/// 作り直しのジョブの種類。
pub const JOB_KIND: &str = "render";
const MAX_ATTEMPTS: i32 = 3;
/// 1回に読む版の数。
const BATCH: i64 = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Job {
    workspace_id: Uuid,
}

/// ワークスペースの作り直しのジョブを積む（まだ動いていない同じジョブがあれば、それにまとめる）。
pub async fn schedule(pool: &PgPool, keys: &KeyRing, workspace_id: Uuid) -> Result<()> {
    crate::jobs::enqueue_debounced(
        pool,
        keys,
        Some(workspace_id),
        JOB_KIND,
        &serde_json::to_vec(&Job { workspace_id })?,
        MAX_ATTEMPTS,
        &format!("{JOB_KIND}:{workspace_id}"),
        0.0,
    )
    .await
}

/// スキーマを適用済みのすべてのワークスペースに積む（サーバーの起動のとき）。
pub async fn schedule_all(pool: &PgPool, keys: &KeyRing) -> Result<()> {
    let ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM cms.workspaces WHERE schema_version IS NOT NULL")
            .fetch_all(pool)
            .await?;
    for id in ids {
        schedule(pool, keys, id).await?;
    }
    Ok(())
}

/// ジョブを1つ実行する。
pub async fn run(pool: &PgPool, engine: &Engine, payload: &[u8]) -> Result<Outcome> {
    let Ok(job) = serde_json::from_slice::<Job>(payload) else {
        return Ok(Outcome::Fail("invalid_payload"));
    };
    let (Some(applied), Some(db)) = (
        applied_schema(pool, job.workspace_id).await?,
        workspace_db::lookup(pool, job.workspace_id).await?,
    ) else {
        return Ok(Outcome::Done);
    };
    let scripts = Scripts {
        engine,
        host: Host::new(pool.clone(), db.clone()),
    };
    let mut after = Uuid::nil();
    let mut failed = false;
    loop {
        let mut tx = workspace_db::begin(pool, &db).await?;
        let rows: Vec<(Uuid, String, String)> = sqlx::query_as(
            "SELECT v.id, e.content_type, v.fields::text
             FROM entries e JOIN versions v ON v.id = e.published_version_id
             WHERE e.deleted_at IS NULL AND v.fields IS NOT NULL
               AND (v.rendered IS NULL OR v.rendered_with IS DISTINCT FROM $1) AND v.id > $2
             ORDER BY v.id
             LIMIT $3",
        )
        .bind(CONVERTER_VERSION)
        .bind(after)
        .bind(BATCH)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        let Some(last) = rows.last().map(|r| r.0) else {
            break;
        };
        after = last;
        for (version_id, content_type, fields) in rows {
            let Some(ct) = applied.manifest.content_type(&content_type) else {
                continue;
            };
            let fields: Map<String, Value> = match serde_json::from_str(&fields)? {
                Value::Object(m) => m,
                _ => Map::new(),
            };
            let rendered = match publishing::render(&scripts, &applied.manifest, ct, &fields).await
            {
                Ok(r) => r,
                Err(f) => {
                    tracing::warn!(workspace_id = %job.workspace_id, %version_id, "cannot re-render a published version: {}", f.code());
                    failed = true;
                    continue;
                }
            };
            let mut tx = workspace_db::begin(pool, &db).await?;
            // 待っている間に本文が抹消されていたら書かない
            sqlx::query(
                "UPDATE versions SET rendered = $2::jsonb, rendered_with = $3 WHERE id = $1 AND fields IS NOT NULL",
            )
            .bind(version_id)
            .bind(rendered.to_string())
            .bind(CONVERTER_VERSION)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
        }
    }
    Ok(if failed {
        Outcome::Retry("conversion_failed")
    } else {
        Outcome::Done
    })
}
