//! ジョブを実行する常駐の処理。サーバーの起動中、キューを見て1つずつ実行する。

use std::time::Duration;

use anyhow::Result;

use crate::http::AppState;
use crate::jobs::{self, Outcome};
use crate::mail;
use crate::webhooks;

/// キューが空のときに、次に見るまで待つ時間。
const IDLE_WAIT: Duration = Duration::from_secs(1);

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
pub fn spawn(state: AppState) -> tokio::task::JoinHandle<()> {
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
}

/// 保存期間を過ぎたログインの履歴を、設定の間隔で削除する常駐の処理（INV-AUD-07）。
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
        }
    })
}
