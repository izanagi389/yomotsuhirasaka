//! 監査ログの外部への書き出し（11-permissions-and-audit.md「外部への書き出し」、Q81、計画の14番）。
//!
//! - 書き出し先は2段。インスタンス全体（設定ファイル。標準出力・ファイル・Webhook）にはすべての記録を、
//!   ワークスペースの書き出し先（Webhook。そのワークスペースの管理者が作る）にはそのワークスペースの記録だけを出す
//!   （原則6、INV-AUD-03）。
//! - 記録した時点では送らない。常駐の処理が、書き出し先ごとに「どこまで送ったか」（記録の時刻とIDの組）を覚えて、
//!   記録の順に最大100件ずつ送る。失敗したらやり直す（原則5）。複数のサーバーが動いていても二重に送らないよう、
//!   送る間は書き出し先の行をロックする。
//! - 記録の時刻はトランザクションの中で決まるので、確定が前後しても取りこぼさないよう、5秒より前の記録だけを送る。
//! - 1行は `audit::line` の形（内容は入らない。INV-AUD-01）。Webhook は `application/x-ndjson` で、記事の Webhook と同じ
//!   形式の署名を付ける（署名のIDは、まとめて送る最初の記録のID。やり直しても変わらない）。
//! - 止まったら、ログと監査ログ（管理者への通知）に残す。設定ファイルの書き出し先は `yomi-server resume-audit-export` で再開する。
//! - 7日間続けて送れなければ「停止」にする。停止した書き出し先は飛ばし、管理者が再開するまで送らない。
//! - 設定ファイルから外した書き出し先は、起動のときに止める（保存期間を過ぎた記録の削除を待たせない）。進み具合は残すので、
//!   同じ書き出し先を戻すと、次の起動で続きから送る。複数のサーバーは同じ設定で動かす前提。
//! - Webhook の送信は、外部への送信の共通の仕組み（egress）だけを通す。

use std::time::Duration;

use anyhow::{Context as _, Result};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::audit::{self, Entry};
use crate::config::AuditExportConfig;
use crate::egress::{Egress, EgressRequest};
use crate::http::AppState;
use crate::keys::KeyRing;
use crate::webhooks;

/// 1回に送る件数の上限。
pub const BATCH: i64 = 100;
/// 書き出し先を見る間隔。
const TICK: Duration = Duration::from_secs(2);
/// 失敗した書き出し先を、次に試すまでの間隔（秒）。
const RETRY_SECONDS: f64 = 60.0;
/// 続けて失敗したら停止にするまでの時間（秒）。
const STOP_AFTER_SECONDS: f64 = 7.0 * 24.0 * 60.0 * 60.0;
/// 1回の見回りで、1つの書き出し先に送るまとまりの数の上限（ほかの書き出し先を待たせないため）。
const MAX_BATCHES_PER_TICK: usize = 10;

/// 設定ファイルから外して止めた印（`last_error` に入れる）。送れずに止まったものと区別する。
const REMOVED_FROM_CONFIG: &str = "removed_from_config";

/// 秘密の暗号化のAAD（書き出し先ごと）。
pub fn secret_aad(sink_id: Uuid) -> Vec<u8> {
    format!("yomi:audit-sink-secret:{sink_id}").into_bytes()
}

/// 書き出し先。
enum Target<'a> {
    Stdout,
    File(&'a std::path::Path),
    /// URL と、署名の秘密（入れ替え中でも1つ）。
    Webhook {
        url: String,
        secret: Vec<u8>,
    },
}

/// 進み具合の行（インスタンスのものは sink_key、ワークスペースのものは書き出し先のID）。
enum Cursor<'a> {
    Instance(&'a str),
    Workspace(Uuid),
}

/// 進み具合（記録の時刻とID）。時刻は文字列のまま DB の比較に戻す（Rust 側では解釈しない）。
type CursorRow = (String, Uuid);

/// 送る記録（進み具合より後で、5秒より前のもの）。
async fn next_batch(
    tx: &mut Transaction<'_, Postgres>,
    workspace: Option<Uuid>,
    cursor: &CursorRow,
) -> Result<Vec<Entry>> {
    let rows: Vec<audit::Row> = sqlx::query_as(crate::sql!(
        r#"SELECT {}
           FROM yomi.audit_log a
           WHERE ($1::uuid IS NULL OR a.workspace_id = $1)
             AND (a.created_at, a.id) > ($2::timestamptz, $3)
             AND a.created_at < now() - interval '5 seconds'
           ORDER BY a.created_at, a.id
           LIMIT $4"#,
        audit::COLUMNS
    ))
    .bind(workspace)
    .bind(&cursor.0)
    .bind(cursor.1)
    .bind(BATCH)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(audit::entry).collect())
}

/// 1行1件の JSON。
fn ndjson(entries: &[Entry]) -> Result<String> {
    let mut out = String::new();
    for e in entries {
        out.push_str(&serde_json::to_string(&audit::line(e))?);
        out.push('\n');
    }
    Ok(out)
}

/// 送る。失敗の理由のコードを返す（内容や相手の応答の本文は含めない）。
async fn send(
    target: &Target<'_>,
    egress: Option<&Egress>,
    pool: &PgPool,
    entries: &[Entry],
) -> Result<(), String> {
    let body = ndjson(entries).map_err(|_| "encode".to_owned())?;
    match target {
        Target::Stdout => {
            use std::io::Write as _;
            let mut out = std::io::stdout().lock();
            out.write_all(body.as_bytes())
                .and_then(|()| out.flush())
                .map_err(|_| "stdout".to_owned())
        }
        Target::File(path) => {
            use tokio::io::AsyncWriteExt as _;
            let mut f = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .await
                .map_err(|_| "file_open".to_owned())?;
            f.write_all(body.as_bytes())
                .await
                .map_err(|_| "file_write".to_owned())?;
            f.flush().await.map_err(|_| "file_write".to_owned())
        }
        Target::Webhook { url, secret } => {
            let egress = egress.ok_or_else(|| "egress_unavailable".to_owned())?;
            let timestamp: i64 = sqlx::query_scalar("SELECT extract(epoch FROM now())::bigint")
                .fetch_one(pool)
                .await
                .map_err(|_| "internal_error".to_owned())?;
            let first = entries.first().map_or(Uuid::nil(), |e| e.id);
            let signature =
                webhooks::signature_header(&[secret.as_slice()], timestamp, first, &body);
            let request = EgressRequest {
                method: hyper::Method::POST,
                url: url.clone(),
                headers: vec![
                    (
                        hyper::header::CONTENT_TYPE,
                        hyper::header::HeaderValue::from_static("application/x-ndjson"),
                    ),
                    (
                        hyper::header::USER_AGENT,
                        hyper::header::HeaderValue::from_static("yomi-audit/1"),
                    ),
                    (
                        hyper::header::HeaderName::from_static(webhooks::SIGNATURE_HEADER),
                        hyper::header::HeaderValue::from_str(&signature)
                            .map_err(|_| "signature".to_owned())?,
                    ),
                ],
                body: body.into_bytes(),
                read_body_limit: None,
            };
            match egress.send(request).await {
                Ok(res) if (200..300).contains(&res.status) => Ok(()),
                Ok(_) => Err("http_status".to_owned()),
                Err(e) => Err(e.code().to_owned()),
            }
        }
    }
}

/// 進み具合の行をロックして読む（ほかのサーバーが送っている・停止・やり直しを待つ間は None）。
async fn lock_cursor(
    tx: &mut Transaction<'_, Postgres>,
    cursor: &Cursor<'_>,
) -> Result<Option<CursorRow>> {
    let row = match cursor {
        Cursor::Instance(key) => {
            sqlx::query_as(
                r#"SELECT to_char(cursor_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'), cursor_id
                   FROM yomi.audit_export_cursors
                   WHERE sink_key = $1 AND stopped_at IS NULL
                     AND (failing_since IS NULL OR last_attempt_at < now() - make_interval(secs => $2))
                   FOR UPDATE SKIP LOCKED"#,
            )
            .bind(*key)
            .bind(RETRY_SECONDS)
            .fetch_optional(&mut **tx)
            .await?
        }
        Cursor::Workspace(id) => {
            sqlx::query_as(
                r#"SELECT to_char(cursor_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'), cursor_id
                   FROM yomi.audit_sinks
                   WHERE id = $1 AND stopped_at IS NULL
                     AND (failing_since IS NULL OR last_attempt_at < now() - make_interval(secs => $2))
                   FOR UPDATE SKIP LOCKED"#,
            )
            .bind(*id)
            .bind(RETRY_SECONDS)
            .fetch_optional(&mut **tx)
            .await?
        }
    };
    Ok(row)
}

/// 成功: 進み具合を進め、失敗の記録を消す。
const ADVANCE_INSTANCE: &str = "UPDATE yomi.audit_export_cursors
     SET cursor_at = $2::timestamptz, cursor_id = $3, failing_since = NULL, last_error = NULL, last_attempt_at = now()
     WHERE sink_key = $1";
const ADVANCE_WORKSPACE: &str = "UPDATE yomi.audit_sinks
     SET cursor_at = $2::timestamptz, cursor_id = $3, failing_since = NULL, last_error = NULL, last_attempt_at = now()
     WHERE id = $1::uuid";
/// 失敗: 理由を記録し、続いて7日を過ぎたら停止にする。
const FAIL_INSTANCE: &str = "UPDATE yomi.audit_export_cursors
     SET failing_since = COALESCE(failing_since, now()), last_error = $2, last_attempt_at = now(),
         stopped_at = CASE WHEN COALESCE(failing_since, now()) < now() - make_interval(secs => $3) THEN now() END
     WHERE sink_key = $1
     RETURNING stopped_at IS NOT NULL";
const FAIL_WORKSPACE: &str = "UPDATE yomi.audit_sinks
     SET failing_since = COALESCE(failing_since, now()), last_error = $2, last_attempt_at = now(),
         stopped_at = CASE WHEN COALESCE(failing_since, now()) < now() - make_interval(secs => $3) THEN now() END
     WHERE id = $1::uuid
     RETURNING stopped_at IS NOT NULL";

/// 送った結果を記録する。
async fn record(
    tx: &mut Transaction<'_, Postgres>,
    cursor: &Cursor<'_>,
    workspace: Option<Uuid>,
    last: &Entry,
    error: Option<&str>,
) -> Result<()> {
    let (key, advance, fail) = match cursor {
        Cursor::Instance(k) => ((*k).to_owned(), ADVANCE_INSTANCE, FAIL_INSTANCE),
        Cursor::Workspace(id) => (id.to_string(), ADVANCE_WORKSPACE, FAIL_WORKSPACE),
    };
    match error {
        None => {
            sqlx::query(advance)
                .bind(&key)
                .bind(&last.created_at)
                .bind(last.id)
                .execute(&mut **tx)
                .await?;
        }
        Some(code) => {
            let stopped: bool = sqlx::query_scalar(fail)
                .bind(&key)
                .bind(code)
                .bind(STOP_AFTER_SECONDS)
                .fetch_one(&mut **tx)
                .await?;
            // 停止した行は `lock_cursor` で読まないので、止まったときに1回だけここに来る
            if stopped {
                record_stopped(tx, cursor, workspace, code).await?;
            }
        }
    }
    Ok(())
}

/// 書き出し先が止まったことを、ログと監査ログ（管理者への通知）に残す。
/// 設定ファイルの書き出し先の名前は URL を含むので、ログには種類だけを出す。
async fn record_stopped(
    tx: &mut Transaction<'_, Postgres>,
    cursor: &Cursor<'_>,
    workspace: Option<Uuid>,
    code: &str,
) -> Result<()> {
    let sink = match cursor {
        Cursor::Instance(key) => key.split(':').next().unwrap_or("instance").to_owned(),
        Cursor::Workspace(id) => id.to_string(),
    };
    tracing::error!(sink = %sink, error = %code, "audit export: a destination failed for 7 days and was stopped");
    let targets: Vec<Uuid> = match cursor {
        Cursor::Instance(_) => Vec::new(),
        Cursor::Workspace(id) => vec![*id],
    };
    audit::record(
        &mut **tx,
        workspace,
        None,
        audit::action::AUDIT_EXPORT_STOPPED,
        &targets,
    )
    .await
}

/// 止まっている設定ファイルの書き出し先を再開する（サーバー上のコマンドだけが呼ぶ）。今の設定にあるものだけを対象にする。
/// 続きの記録から送る。止まっている間に保存期間を過ぎて消えた記録は送れない。再開した書き出し先の名前を返す。
pub async fn resume_instance_sinks(
    pool: &PgPool,
    config: &AuditExportConfig,
) -> Result<Vec<String>> {
    let mut tx = pool.begin().await?;
    let mut resumed: Vec<String> = sqlx::query_scalar(
        "UPDATE yomi.audit_export_cursors
         SET stopped_at = NULL, failing_since = NULL, last_error = NULL
         WHERE sink_key = ANY($1) AND stopped_at IS NOT NULL
         RETURNING sink_key",
    )
    .bind(instance_sinks(config))
    .fetch_all(&mut *tx)
    .await?;
    resumed.sort();
    if !resumed.is_empty() {
        audit::record(
            &mut *tx,
            None,
            None,
            audit::action::AUDIT_EXPORT_RESUMED,
            &[],
        )
        .await?;
    }
    tx.commit().await?;
    Ok(resumed)
}

/// 1つの書き出し先に、たまっている記録を送る。
async fn drain(
    state: &AppState,
    cursor: Cursor<'_>,
    workspace: Option<Uuid>,
    target: &Target<'_>,
) -> Result<()> {
    for _ in 0..MAX_BATCHES_PER_TICK {
        let mut tx = state.pool.begin().await?;
        let Some(row) = lock_cursor(&mut tx, &cursor).await? else {
            return Ok(());
        };
        let entries = next_batch(&mut tx, workspace, &row).await?;
        let Some(last) = entries.last() else {
            return Ok(());
        };
        let result = send(target, state.egress.as_ref(), &state.pool, &entries).await;
        let failed = result.as_ref().err().cloned();
        record(&mut tx, &cursor, workspace, last, failed.as_deref()).await?;
        tx.commit().await?;
        if let Some(code) = failed {
            tracing::warn!(error = %code, "audit export: a destination did not accept records");
            return Ok(());
        }
        if (entries.len() as i64) < BATCH {
            return Ok(());
        }
    }
    Ok(())
}

/// インスタンス全体の書き出し先の名前（進み具合の行のキー）。
fn instance_sinks(config: &AuditExportConfig) -> Vec<String> {
    let mut out = Vec::new();
    if config.stdout {
        out.push("stdout".to_owned());
    }
    if let Some(path) = &config.file {
        out.push(format!("file:{}", path.display()));
    }
    for hook in &config.webhooks {
        out.push(format!("webhook:{}", hook.url));
    }
    out
}

/// インスタンス全体の書き出し先の進み具合の行を、今の設定に合わせる（サーバーの起動のときに呼ぶ）。
/// - 初めて設定したものは、その時点から送る
/// - 設定から外したものは止める（`purge_audit_log` は止まった書き出し先を待たない）
/// - 外して止めたものが設定に戻ったら、続きから送る。送れずに止まったものは、戻しても止まったまま（コマンドで再開する）
pub async fn register_instance_sinks(pool: &PgPool, config: &AuditExportConfig) -> Result<()> {
    let keys = instance_sinks(config);
    let mut tx = pool.begin().await?;
    let removed = sqlx::query(
        "UPDATE yomi.audit_export_cursors
         SET stopped_at = now(), failing_since = NULL, last_error = $2
         WHERE sink_key <> ALL($1) AND stopped_at IS NULL",
    )
    .bind(&keys)
    .bind(REMOVED_FROM_CONFIG)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let restored = sqlx::query(
        "UPDATE yomi.audit_export_cursors
         SET stopped_at = NULL, last_error = NULL
         WHERE sink_key = ANY($1) AND last_error = $2",
    )
    .bind(&keys)
    .bind(REMOVED_FROM_CONFIG)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    for key in &keys {
        sqlx::query(
            "INSERT INTO yomi.audit_export_cursors (sink_key) VALUES ($1) ON CONFLICT DO NOTHING",
        )
        .bind(key)
        .execute(&mut *tx)
        .await?;
    }
    // 書き出し先の名前は URL を含むので、ログと監査ログには出さない
    if removed > 0 {
        tracing::warn!(
            count = removed,
            "audit export: destinations removed from the config were stopped"
        );
        audit::record(
            &mut *tx,
            None,
            None,
            audit::action::AUDIT_EXPORT_REMOVED,
            &[],
        )
        .await?;
    }
    if restored > 0 {
        audit::record(
            &mut *tx,
            None,
            None,
            audit::action::AUDIT_EXPORT_RESUMED,
            &[],
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// 1回の見回り。
pub async fn tick(state: &AppState) -> Result<()> {
    let config = &state.config.audit_export;
    if config.stdout {
        drain(state, Cursor::Instance("stdout"), None, &Target::Stdout).await?;
    }
    if let Some(path) = &config.file {
        let key = format!("file:{}", path.display());
        drain(state, Cursor::Instance(&key), None, &Target::File(path)).await?;
    }
    for hook in &config.webhooks {
        let key = format!("webhook:{}", hook.url);
        // 秘密のファイルは送るたびに読む（入れ替えたら、再起動しなくても新しい秘密で署名する）
        let secret = tokio::fs::read_to_string(&hook.secret_file)
            .await
            .with_context(|| format!("cannot read the secret file of {key}"))?;
        let target = Target::Webhook {
            url: hook.url.clone(),
            secret: secret.trim().as_bytes().to_vec(),
        };
        drain(state, Cursor::Instance(&key), None, &target).await?;
    }
    let sinks: Vec<(Uuid, Uuid, String, Vec<u8>)> = sqlx::query_as(
        "SELECT id, workspace_id, url, secret_enc FROM yomi.audit_sinks WHERE stopped_at IS NULL ORDER BY created_at",
    )
    .fetch_all(&state.pool)
    .await?;
    for (id, workspace_id, url, secret_enc) in sinks {
        let Ok(secret) = state.keys.decrypt(&secret_enc, &secret_aad(id)) else {
            tracing::error!(sink_id = %id, "audit export: cannot decrypt the secret of a workspace destination");
            continue;
        };
        drain(
            state,
            Cursor::Workspace(id),
            Some(workspace_id),
            &Target::Webhook { url, secret },
        )
        .await?;
    }
    Ok(())
}

/// 書き出しの常駐の処理を始める。
pub fn spawn(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = register_instance_sinks(&state.pool, &state.config.audit_export).await {
            tracing::error!("audit export: cannot register the destinations: {e:#}");
        }
        let mut ticker = tokio::time::interval(TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if let Err(e) = tick(&state).await {
                tracing::error!("audit export: {e:#}");
            }
        }
    })
}

/// ワークスペースの書き出し先の秘密を作り、暗号化したものと一緒に返す。
pub fn new_secret(keys: &KeyRing, sink_id: Uuid) -> Result<(String, Vec<u8>)> {
    let secret = webhooks::generate_secret();
    let sealed = keys.encrypt(secret.as_bytes(), &secret_aad(sink_id))?;
    Ok((secret, sealed))
}
