//! ジョブを実行する常駐の処理。サーバーの起動中、キューを見て1つずつ実行する。

use std::time::Duration;

use anyhow::Result;

use crate::http::AppState;
use crate::jobs::{self, Outcome};
use crate::mail;

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
