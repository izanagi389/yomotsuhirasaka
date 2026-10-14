//! Webhook（01-architecture.md「Webhook」、13-security.md「外部への送信」「Webhookの署名」）。
//!
//! - 記事の状態変化をイベントとして、ワークスペースに登録したすべての送信先に送る。共有キャッシュの有無にかかわらず送る（Q146）。
//! - 本文には、記事の本文や下書きの内容を含めない。ワークスペースID・記事ID・翻訳グループID・ロケール・
//!   イベントの種類・公開URLだけを含める（原則7、Q160、INV-WH-07）。
//! - 送信はジョブで行い、失敗したらやり直す（原則5）。送信は外部への送信の共通の仕組み（`egress`）だけを通す。
//! - 署名は HMAC-SHA256。署名の対象は `<送信時刻>.<イベントID>.<本文>`。送信時刻は試行ごとに付け直し、
//!   イベントIDはやり直しても変えない（受け手が古いものや同じものの再送を拒否できるように。INV-WH-03）。
//!   ヘッダーの書式: `Webhook-Signature: t=<UNIX秒>,id=<イベントID>,v1=<16進>[,v1=<16進>]`
//!   （秘密の入れ替え中は、新旧それぞれの秘密で作った `v1` を並べる）。
//! - 相手の応答は状態コードだけを記録し、本文は読まない（Q112、INV-WH-04）。

use anyhow::{Context as _, Result};
use hmac::{KeyInit as _, Mac as _};
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::egress::{Egress, EgressError, EgressRequest};
use crate::jobs::{self, Outcome};
use crate::keys::KeyRing;

/// ジョブの種類。
pub const JOB_KIND: &str = "webhook";
/// 1つの送信の試行の回数の上限（30秒から倍々で、最後は1時間おき）。
const MAX_ATTEMPTS: i32 = 10;
/// 秘密を入れ替えた後、古い秘密でも署名する時間（秒）。
pub const PREVIOUS_SECRET_SECONDS: i64 = 24 * 60 * 60;
/// 署名のヘッダーの名前。
pub const SIGNATURE_HEADER: &str = "webhook-signature";
/// 秘密の接頭辞。
const SECRET_PREFIX: &str = "whsec_";

/// 記事の状態変化のイベント（01-architecture.md「Webhook」、Q146）。名前は受け手が使うので変えない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryEvent {
    Published,
    Updated,
    Unpublished,
    Deleted,
    /// 訂正として公開（Q48）。
    Corrected,
    /// 版の本文の抹消（Q144）。
    VersionErased,
    /// 公開設定の変更（Q184）。
    SettingsChanged,
    /// 公開URLの変更。
    UrlChanged,
}

impl EntryEvent {
    pub fn name(self) -> &'static str {
        match self {
            Self::Published => "entry.published",
            Self::Updated => "entry.updated",
            Self::Unpublished => "entry.unpublished",
            Self::Deleted => "entry.deleted",
            Self::Corrected => "entry.corrected",
            Self::VersionErased => "entry.version_erased",
            Self::SettingsChanged => "entry.settings_changed",
            Self::UrlChanged => "entry.url_changed",
        }
    }
}

/// テスト送信のイベントの種類。
pub const TEST_EVENT: &str = "webhook.test";

/// イベントの対象の記事（本文は含めない）。
#[derive(Debug, Clone)]
pub struct EntryRef {
    pub id: Uuid,
    pub translation_group_id: Uuid,
    pub locale: String,
    /// 公開URL（最後に公開したときのもの。一度も公開していなければ None）。
    pub url: Option<String>,
}

/// 記事の参照を読む（削除済みの記事も読む）。ワークスペースのロールのトランザクションの中で呼ぶ。
pub async fn entry_ref(conn: &mut PgConnection, entry_id: Uuid) -> Result<Option<EntryRef>> {
    let row: Option<(Uuid, Uuid, String, Option<String>)> =
        sqlx::query_as("SELECT id, translation_group_id, locale, url FROM entries WHERE id = $1")
            .bind(entry_id)
            .fetch_optional(conn)
            .await?;
    Ok(row.map(|(id, translation_group_id, locale, url)| EntryRef {
        id,
        translation_group_id,
        locale,
        url,
    }))
}

/// Webhookの本文。ここにない値は送らない（INV-WH-07）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Body<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    workspace_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    entry_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    translation_group_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    locale: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<&'a str>,
}

/// ジョブの中身（暗号化して保存される）。
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Job {
    delivery_id: Uuid,
    webhook_id: Uuid,
    event_id: Uuid,
    body: String,
}

/// 秘密の暗号化のAAD（Webhookごと）。
pub fn secret_aad(webhook_id: Uuid) -> Vec<u8> {
    format!("cms:webhook-secret:{webhook_id}").into_bytes()
}

/// 新しい署名用の秘密。
pub fn generate_secret() -> String {
    format!("{SECRET_PREFIX}{}", crate::auth::token::generate())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// 署名のヘッダーの値。`secrets` は新しい順（入れ替え中は新旧の2つ）。HMACの鍵は秘密の文字列のバイト列。
pub fn signature_header(secrets: &[&[u8]], timestamp: i64, event_id: Uuid, body: &str) -> String {
    let input = format!("{timestamp}.{event_id}.{body}");
    let mut out = format!("t={timestamp},id={event_id}");
    for secret in secrets {
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret)
            .expect("HMAC accepts keys of any length");
        mac.update(input.as_bytes());
        out.push_str(",v1=");
        out.push_str(&hex(&mac.finalize().into_bytes()));
    }
    out
}

/// 記事のイベントを、ワークスペースのすべての送信先に送る（ジョブを積む）。
/// 操作を確定（commit）した後に呼ぶ。
pub async fn emit(
    pool: &PgPool,
    keys: &KeyRing,
    workspace_id: Uuid,
    events: &[EntryEvent],
    entry: &EntryRef,
) -> Result<()> {
    for event in events {
        let body = serde_json::to_string(&Body {
            kind: event.name(),
            workspace_id,
            entry_id: Some(entry.id),
            translation_group_id: Some(entry.translation_group_id),
            locale: Some(&entry.locale),
            url: entry.url.as_deref(),
        })?;
        enqueue_all(
            pool,
            keys,
            workspace_id,
            event.name(),
            Some(entry.id),
            &body,
        )
        .await?;
    }
    Ok(())
}

/// すべての送信先にジョブを積む（1つのイベントに1つのイベントID）。
async fn enqueue_all(
    pool: &PgPool,
    keys: &KeyRing,
    workspace_id: Uuid,
    event_type: &str,
    entry_id: Option<Uuid>,
    body: &str,
) -> Result<()> {
    let event_id = Uuid::new_v4();
    let mut tx = pool.begin().await?;
    let hooks: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM cms.webhooks WHERE workspace_id = $1 ORDER BY created_at",
    )
    .bind(workspace_id)
    .fetch_all(&mut *tx)
    .await?;
    for webhook_id in hooks {
        enqueue_one(
            &mut tx,
            keys,
            workspace_id,
            webhook_id,
            event_id,
            event_type,
            entry_id,
            body,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_one(
    conn: &mut PgConnection,
    keys: &KeyRing,
    workspace_id: Uuid,
    webhook_id: Uuid,
    event_id: Uuid,
    event_type: &str,
    entry_id: Option<Uuid>,
    body: &str,
) -> Result<Uuid> {
    let delivery_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO cms.webhook_deliveries (id, webhook_id, workspace_id, event_id, event_type, entry_id)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(delivery_id)
    .bind(webhook_id)
    .bind(workspace_id)
    .bind(event_id)
    .bind(event_type)
    .bind(entry_id)
    .execute(&mut *conn)
    .await?;
    let job = serde_json::to_vec(&Job {
        delivery_id,
        webhook_id,
        event_id,
        body: body.to_owned(),
    })?;
    jobs::enqueue(
        &mut *conn,
        keys,
        Some(workspace_id),
        JOB_KIND,
        &job,
        MAX_ATTEMPTS,
    )
    .await?;
    Ok(delivery_id)
}

/// テスト送信（1つの送信先に、テストのイベントを送るジョブを積む）。送信の記録のIDを返す。
pub async fn enqueue_test(
    pool: &PgPool,
    keys: &KeyRing,
    workspace_id: Uuid,
    webhook_id: Uuid,
) -> Result<Uuid> {
    let body = serde_json::to_string(&Body {
        kind: TEST_EVENT,
        workspace_id,
        entry_id: None,
        translation_group_id: None,
        locale: None,
        url: None,
    })?;
    let mut tx = pool.begin().await?;
    let id = enqueue_one(
        &mut tx,
        keys,
        workspace_id,
        webhook_id,
        Uuid::new_v4(),
        TEST_EVENT,
        None,
        &body,
    )
    .await?;
    tx.commit().await?;
    Ok(id)
}

/// 送信先として使えない（やり直しても変わらない）失敗か。
fn permanent(e: EgressError) -> bool {
    matches!(
        e,
        EgressError::InvalidUrl | EgressError::SchemeNotAllowed | EgressError::InternalDestination
    )
}

type HookRow = (String, Vec<u8>, Option<Vec<u8>>, bool);

/// ジョブを1つ実行する（送る）。
pub async fn deliver(
    pool: &PgPool,
    keys: &KeyRing,
    egress: Option<&Egress>,
    payload: &[u8],
    attempt: i32,
    max_attempts: i32,
) -> Result<Outcome> {
    let Ok(job) = serde_json::from_slice::<Job>(payload) else {
        return Ok(Outcome::Fail("invalid_payload"));
    };
    // 送るたびに、現在の送信先と秘密を読む（削除されていれば送らない）
    let hook: Option<HookRow> = sqlx::query_as(
        "SELECT url, secret_enc, previous_secret_enc, coalesce(previous_secret_until > now(), false)
         FROM cms.webhooks WHERE id = $1",
    )
    .bind(job.webhook_id)
    .fetch_optional(pool)
    .await?;
    let Some((url, secret_enc, previous_enc, previous_valid)) = hook else {
        return Ok(Outcome::Done);
    };
    let aad = secret_aad(job.webhook_id);
    let Ok(secret) = keys.decrypt(&secret_enc, &aad) else {
        return record(
            pool,
            &job,
            None,
            Outcome::Fail("cannot_decrypt_secret"),
            attempt,
            max_attempts,
        )
        .await;
    };
    let previous = match (previous_enc, previous_valid) {
        (Some(p), true) => keys.decrypt(&p, &aad).ok(),
        _ => None,
    };
    let mut secrets: Vec<&[u8]> = vec![&secret];
    if let Some(p) = &previous {
        secrets.push(p);
    }
    let Some(egress) = egress else {
        return record(
            pool,
            &job,
            None,
            Outcome::Retry("egress_unavailable"),
            attempt,
            max_attempts,
        )
        .await;
    };
    let timestamp: i64 = sqlx::query_scalar("SELECT extract(epoch FROM now())::bigint")
        .fetch_one(pool)
        .await?;
    let signature = signature_header(&secrets, timestamp, job.event_id, &job.body);
    let request = EgressRequest {
        method: hyper::Method::POST,
        url,
        headers: vec![
            (
                hyper::header::CONTENT_TYPE,
                hyper::header::HeaderValue::from_static("application/json"),
            ),
            (
                hyper::header::USER_AGENT,
                hyper::header::HeaderValue::from_static("cms-webhook/1"),
            ),
            (
                hyper::header::HeaderName::from_static(SIGNATURE_HEADER),
                hyper::header::HeaderValue::from_str(&signature).context("signature header")?,
            ),
        ],
        body: job.body.clone().into_bytes(),
        read_body_limit: None,
    };
    let (status, outcome) = match egress.send(request).await {
        Ok(res) if (200..300).contains(&res.status) => (Some(res.status), Outcome::Done),
        Ok(res) => (Some(res.status), Outcome::Retry("http_status")),
        Err(e) if permanent(e) => (None, Outcome::Fail(e.code())),
        Err(e) => (None, Outcome::Retry(e.code())),
    };
    record(pool, &job, status, outcome, attempt, max_attempts).await
}

/// 送信の記録を更新する。
async fn record(
    pool: &PgPool,
    job: &Job,
    status: Option<u16>,
    outcome: Outcome,
    attempt: i32,
    max_attempts: i32,
) -> Result<Outcome> {
    let (state, error) = match outcome {
        Outcome::Done => ("delivered", None),
        Outcome::Retry(e) if attempt < max_attempts => ("pending", Some(e)),
        Outcome::Retry(e) | Outcome::Fail(e) => ("failed", Some(e)),
    };
    sqlx::query(
        "UPDATE cms.webhook_deliveries
         SET state = $2, attempts = $3, last_status = $4, last_error = $5, last_attempt_at = now()
         WHERE id = $1",
    )
    .bind(job.delivery_id)
    .bind(state)
    .bind(attempt)
    .bind(status.map(i32::from))
    .bind(error)
    .execute(pool)
    .await?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_covers_time_id_and_body() {
        let id = Uuid::parse_str("6f1d6a2e-6d5b-4c0e-9c3f-1a2b3c4d5e6f").unwrap();
        let h = signature_header(&[b"whsec_a"], 1_700_000_000, id, "{\"x\":1}");
        // 期待値は独立に計算したもの: HMAC-SHA256("whsec_a", "1700000000.<id>.{\"x\":1}")
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(b"whsec_a").unwrap();
        mac.update(format!("1700000000.{id}.{{\"x\":1}}").as_bytes());
        let expected = hex(&mac.finalize().into_bytes());
        assert_eq!(h, format!("t=1700000000,id={id},v1={expected}"));
        assert_ne!(
            h,
            signature_header(&[b"whsec_a"], 1_700_000_001, id, "{\"x\":1}")
        );
        let two = signature_header(&[b"whsec_a", b"whsec_b"], 1, id, "b");
        assert_eq!(two.matches(",v1=").count(), 2);
    }

    #[test]
    fn hex_is_lowercase() {
        assert_eq!(hex(&[0x00, 0xab, 0xff]), "00abff");
    }
}
