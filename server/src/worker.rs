//! ジョブを実行する常駐の処理。サーバーの起動中、キューを見て実行する。
//! 時間のかかるフックのジョブがメールやWebhookを待たせないよう、複数の取り出し手で並べて動かす
//! （フックのジョブは、ワークスペースごとに1つずつ。`jobs::claim`）。

use std::time::Duration;

use anyhow::Result;

use crate::content::hooks;
use crate::http::AppState;
use crate::jobs::{self, Outcome};
use crate::mail;
use crate::webhooks;

/// キューが空のときに、次に見るまで待つ時間。
const IDLE_WAIT: Duration = Duration::from_secs(1);
/// 並べて動かす取り出し手の数。
const WORKERS: usize = 4;

/// ジョブを1つ取り出して実行する。実行したら true。
pub async fn run_once(state: &AppState) -> Result<bool> {
    let Some(job) = jobs::claim(&state.pool, &state.keys).await? else {
        return Ok(false);
    };
    let outcome = match (&job.payload, job.kind.as_str()) {
        (None, _) => Outcome::Fail("cannot_decrypt_payload"),
        (Some(payload), mail::JOB_KIND) => {
            mail::deliver(state.config.smtp.as_ref(), &state.issuer, payload).await
        }
        (Some(payload), webhooks::JOB_KIND) => webhooks::deliver(
            &state.pool,
            &state.keys,
            state.egress.as_ref(),
            payload,
            job.attempts,
            job.max_attempts,
        )
        .await
        .unwrap_or_else(|e| {
            tracing::error!(job_id = %job.id, "webhook delivery: {e:#}");
            Outcome::Retry("internal_error")
        }),
        (Some(payload), crate::content::urls::JOB_KIND) => {
            crate::content::urls::run(&state.pool, &state.keys, &state.scripts, payload)
                .await
                .unwrap_or_else(|e| {
                    tracing::error!(job_id = %job.id, "URL recomputation: {e:#}");
                    Outcome::Retry("internal_error")
                })
        }
        (Some(payload), crate::http::admin_scheduling::JOB_KIND) => {
            crate::http::admin_scheduling::run(state, payload, job.attempts)
                .await
                .unwrap_or_else(|e| {
                    tracing::error!(job_id = %job.id, "scheduled publication: {e:#}");
                    Outcome::Retry("internal_error")
                })
        }
        (Some(payload), crate::content::rerender::JOB_KIND) => {
            crate::content::rerender::run(&state.pool, &state.scripts, payload)
                .await
                .unwrap_or_else(|e| {
                    tracing::error!(job_id = %job.id, "re-rendering: {e:#}");
                    Outcome::Retry("internal_error")
                })
        }
        (Some(payload), crate::search::index::JOB_KIND) => crate::search::index::run(
            &state.pool,
            &state.keys,
            &state.config,
            state.egress.as_ref(),
            payload,
        )
        .await
        .unwrap_or_else(|e| {
            tracing::error!(job_id = %job.id, "search indexing: {e:#}");
            Outcome::Retry("internal_error")
        }),
        (Some(payload), crate::search::batch::JOB_KIND) => {
            crate::search::batch::run(&state.pool, &state.keys, &state.config, payload)
                .await
                .unwrap_or_else(|e| {
                    tracing::error!(job_id = %job.id, "related entries batch: {e:#}");
                    Outcome::Retry("internal_error")
                })
        }
        (Some(payload), crate::assist::conflicts::JOB_KIND) => {
            crate::assist::conflicts::run(state, payload)
                .await
                .unwrap_or_else(|e| {
                    tracing::error!(job_id = %job.id, "conflict detection: {e:#}");
                    Outcome::Fail("internal_error")
                })
        }
        (Some(payload), hooks::JOB_KIND) => {
            hooks::run(&state.pool, &state.scripts, state.egress.as_ref(), payload)
                .await
                .unwrap_or_else(|e| {
                    tracing::error!(job_id = %job.id, "hook: {e:#}");
                    Outcome::Retry("internal_error")
                })
        }
        (Some(_), _) => Outcome::Fail("unknown_job_kind"),
    };
    if !matches!(outcome, Outcome::Done) {
        tracing::warn!(
            job_id = %job.id,
            kind = %job.kind,
            attempt = job.attempts,
            "job did not complete: {outcome:?}"
        );
    }
    jobs::finish(&state.pool, &job, outcome).await?;
    Ok(true)
}

/// 常駐の処理を始める。
pub fn spawn(state: AppState) -> Vec<tokio::task::JoinHandle<()>> {
    (0..WORKERS)
        .map(|_| {
            let state = state.clone();
            tokio::spawn(async move {
                loop {
                    match run_once(&state).await {
                        Ok(true) => {}
                        Ok(false) => tokio::time::sleep(IDLE_WAIT).await,
                        Err(e) => {
                            tracing::error!("job worker: {e:#}");
                            tokio::time::sleep(IDLE_WAIT).await;
                        }
                    }
                }
            })
        })
        .collect()
}

/// 保存期間を過ぎたログインの履歴と監査ログを、設定の間隔で削除する常駐の処理（INV-AUD-07、計画の14番）。
pub fn spawn_sweeper(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let every = Duration::from_secs(state.config.login_history.sweep_interval_seconds.into());
        let mut ticker = tokio::time::interval(every);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if let Err(e) = crate::auth::devices::sweep(
                &state.pool,
                state.config.login_history.retention_seconds,
            )
            .await
            {
                tracing::error!("login history sweeper: {e:#}");
            }
            if let Err(e) = crate::auth::passkeys::sweep_challenges(&state.pool).await {
                tracing::error!("passkey challenge sweeper: {e:#}");
            }
            if let Err(e) = crate::notifications::sweep(&state.pool).await {
                tracing::error!("notification reads sweeper: {e:#}");
            }
            // 保存期間を過ぎた質問ログ（INV-LOG-02）
            if let Err(e) = crate::answer::log::sweep(&state.pool).await {
                tracing::error!("question log sweeper: {e:#}");
            }
            // 寿命の過ぎた回答キャッシュ
            if let Err(e) = crate::answer::cache::sweep(&state.pool).await {
                tracing::error!("answer cache sweeper: {e:#}");
            }
            // 残す時間を過ぎた矛盾の検知の結果
            if let Err(e) = crate::assist::conflicts::sweep(&state.pool).await {
                tracing::error!("conflict detection sweeper: {e:#}");
            }
            // 保存期間を過ぎた監査ログ（専用のロールの関数だけが消せる。送り終えていない記録は残す）
            match sqlx::query_scalar::<_, i64>("SELECT cms.purge_audit_log($1)")
                .bind(state.config.audit.retention_days)
                .fetch_one(&state.pool)
                .await
            {
                Ok(n) if n > 0 => tracing::info!(
                    deleted = n,
                    "audit log: removed records past the retention period"
                ),
                Ok(_) => {}
                Err(e) => tracing::error!("audit log purge: {e:#}"),
            }
        }
    })
}
