//! ジョブのキュー（PostgreSQL、`FOR UPDATE SKIP LOCKED`）。
//!
//! - 中身は暗号化の鍵で暗号化して保存する。AADにジョブのIDと種類を入れる。
//! - 終わったジョブは消す。送れなかったジョブは中身を消し、失敗の種類だけを残す。
//! - 失敗したら、間隔を延ばしながらやり直す（30秒から2倍ずつ、上限1時間）。

use anyhow::{Context, Result};
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use crate::keys::KeyRing;

/// 取り出したジョブを、他の取り出し手から隠しておく時間（秒）。この間に終わらなければ、やり直しの対象になる。
const LOCK_SECONDS: f64 = 300.0;
const BASE_BACKOFF_SECONDS: f64 = 30.0;
const MAX_BACKOFF_SECONDS: f64 = 3600.0;
/// ワークスペースごとに1つずつ実行するジョブの種類（フック。時間がかかりうるため）。
pub const ONE_AT_A_TIME_PER_WORKSPACE: &str = crate::content::hooks::JOB_KIND;

pub(crate) fn aad(id: Uuid, kind: &str) -> Vec<u8> {
    format!("cms:job:{kind}:{id}").into_bytes()
}

/// ジョブを積む。呼び出し側のトランザクションの中で積めば、操作と同時に確定する。
pub async fn enqueue<'e>(
    executor: impl PgExecutor<'e>,
    keys: &KeyRing,
    workspace_id: Option<Uuid>,
    kind: &str,
    payload: &[u8],
    max_attempts: i32,
) -> Result<Uuid> {
    let id = Uuid::new_v4();
    let sealed = keys.encrypt(payload, &aad(id, kind))?;
    sqlx::query(
        "INSERT INTO cms.jobs (id, workspace_id, kind, payload_enc, max_attempts)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(workspace_id)
    .bind(kind)
    .bind(sealed)
    .bind(max_attempts)
    .execute(executor)
    .await?;
    Ok(id)
}

/// まとめて1回だけ動かすジョブを積む（`delay_seconds` 後に実行）。まだ取り出されていない同じ `dedupe_key` の
/// ジョブがあれば、新しく積まずにその実行の時刻だけを延ばす（中身は最初に積んだもの）。
#[allow(clippy::too_many_arguments)]
pub async fn enqueue_debounced<'e>(
    executor: impl PgExecutor<'e>,
    keys: &KeyRing,
    workspace_id: Option<Uuid>,
    kind: &str,
    payload: &[u8],
    max_attempts: i32,
    dedupe_key: &str,
    delay_seconds: f64,
) -> Result<()> {
    let id = Uuid::new_v4();
    let sealed = keys.encrypt(payload, &aad(id, kind))?;
    sqlx::query(
        "INSERT INTO cms.jobs (id, workspace_id, kind, payload_enc, max_attempts, dedupe_key, run_at)
         VALUES ($1, $2, $3, $4, $5, $6, now() + make_interval(secs => $7))
         ON CONFLICT (dedupe_key) WHERE dedupe_key IS NOT NULL AND attempts = 0 AND failed_at IS NULL
         DO UPDATE SET run_at = EXCLUDED.run_at",
    )
    .bind(id)
    .bind(workspace_id)
    .bind(kind)
    .bind(sealed)
    .bind(max_attempts)
    .bind(dedupe_key)
    .bind(delay_seconds)
    .execute(executor)
    .await?;
    Ok(())
}

/// 取り出したジョブ。
pub struct Claimed {
    pub id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub kind: String,
    /// 復号した中身。復号できなければ None（そのジョブは失敗として扱う）。
    pub payload: Option<Vec<u8>>,
    pub attempts: i32,
    pub max_attempts: i32,
}

type ClaimRow = (Uuid, Option<Uuid>, String, Option<Vec<u8>>, i32, i32);

/// 実行できるジョブを1つ取り出す（他の取り出し手と重ならない）。
/// フックのジョブ（[`ONE_AT_A_TIME_PER_WORKSPACE`]）は、同じワークスペースで実行中のものがあれば後に回す（Q120）。
pub async fn claim(pool: &PgPool, keys: &KeyRing) -> Result<Option<Claimed>> {
    let row: Option<ClaimRow> = sqlx::query_as(
        "UPDATE cms.jobs SET attempts = attempts + 1,
                locked_until = now() + make_interval(secs => $1)
         WHERE id = (
             SELECT id FROM cms.jobs
             WHERE failed_at IS NULL AND run_at <= now()
               AND (locked_until IS NULL OR locked_until < now())
               AND (kind <> $2 OR NOT EXISTS (
                   SELECT 1 FROM cms.jobs running
                   WHERE running.kind = $2 AND running.workspace_id = cms.jobs.workspace_id
                     AND running.locked_until >= now() AND running.failed_at IS NULL))
             ORDER BY run_at
             FOR UPDATE SKIP LOCKED
             LIMIT 1
         )
         RETURNING id, workspace_id, kind, payload_enc, attempts, max_attempts",
    )
    .bind(LOCK_SECONDS)
    .bind(ONE_AT_A_TIME_PER_WORKSPACE)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(id, workspace_id, kind, sealed, attempts, max_attempts)| Claimed {
            payload: sealed.and_then(|s| keys.decrypt(&s, &aad(id, &kind)).ok()),
            id,
            workspace_id,
            kind,
            attempts,
            max_attempts,
        },
    ))
}

/// ジョブの結果。失敗の理由は種類だけ（宛先や相手の応答の本文を含めない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Done,
    /// しばらくしてからやり直す。
    Retry(&'static str),
    /// やり直しても成功しない。
    Fail(&'static str),
}

/// n回目の試行の後に待つ秒数。
pub fn backoff_seconds(attempts: i32) -> f64 {
    let exp = attempts.saturating_sub(1).clamp(0, 30);
    (BASE_BACKOFF_SECONDS * 2f64.powi(exp)).min(MAX_BACKOFF_SECONDS)
}

/// 結果を記録する。
pub async fn finish(pool: &PgPool, job: &Claimed, outcome: Outcome) -> Result<()> {
    match outcome {
        Outcome::Done => {
            sqlx::query("DELETE FROM cms.jobs WHERE id = $1")
                .bind(job.id)
                .execute(pool)
                .await?;
        }
        Outcome::Retry(reason) if job.attempts < job.max_attempts => {
            sqlx::query(
                "UPDATE cms.jobs SET locked_until = NULL, last_error = $2,
                        run_at = now() + make_interval(secs => $3)
                 WHERE id = $1",
            )
            .bind(job.id)
            .bind(reason)
            .bind(backoff_seconds(job.attempts))
            .execute(pool)
            .await?;
        }
        Outcome::Retry(reason) | Outcome::Fail(reason) => {
            sqlx::query(
                "UPDATE cms.jobs SET locked_until = NULL, last_error = $2, failed_at = now(),
                        payload_enc = NULL
                 WHERE id = $1",
            )
            .bind(job.id)
            .bind(reason)
            .execute(pool)
            .await
            .context("cannot record job failure")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_is_capped() {
        assert_eq!(backoff_seconds(1), 30.0);
        assert_eq!(backoff_seconds(2), 60.0);
        assert_eq!(backoff_seconds(3), 120.0);
        assert_eq!(backoff_seconds(8), 3600.0);
        assert_eq!(backoff_seconds(1000), 3600.0);
        assert_eq!(backoff_seconds(0), 30.0);
    }
}
