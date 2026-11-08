//! 暗号化の鍵の入れ替え（13-security.md「鍵の管理」、Q93、INV-KEY-03）の、DBの側の処理。
//!
//! 手順（サーバー上のCLI）:
//! 1. `add-encryption-key`: 鍵のファイルに新しい版の鍵を加える。すべてのサーバーを再起動し、新しい鍵で暗号化させる
//! 2. `reencrypt-secrets`: 保存済みの秘密を、新しい鍵で暗号化し直す（この関数）
//! 3. `remove-encryption-key`: 古い版の鍵を外す。古い版で暗号化された値が残っていれば断る
//!
//! 暗号化して保存している値（増やしたら、ここにも足す）:
//! - `yomi.users.totp_secret_enc`, `yomi.users.totp_pending_secret_enc`（TOTPの秘密）
//! - `yomi.jobs.payload_enc`（ジョブの中身。メールの宛先や本文、Webhookの送信）
//! - `yomi.webhooks.secret_enc`, `yomi.webhooks.previous_secret_enc`（Webhookの署名用の秘密）
//! - `yomi.audit_sinks.secret_enc`（監査ログの書き出し先の署名用の秘密）
//! - `yomi.workspace_llm.api_key`（ワークスペースの LLM の接続設定の上書きの APIキー。M4）
//!
//! 暗号化し直さないもの: `yomi.oidc_logins.pkce_verifier`（SSOのログインの途中の値。10分で失効する）。
//! 古い版の鍵を外した時点で途中だったSSOのログインは、やり直しになる。

use std::collections::BTreeMap;

use anyhow::Result;
use sqlx::PgPool;
use uuid::Uuid;

use crate::keys::{KeyRing, version_of};

/// 1つのトランザクションで暗号化し直す行の数。
const BATCH: usize = 200;

/// 暗号化し直した結果。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// 暗号化し直した値の数。
    pub reencrypted: u64,
    /// 復号できなかった値の数（鍵のファイルにない版など）。変更せずに残す。
    pub failed: u64,
}

/// 暗号化された値が、どの版の鍵でいくつあるか。
pub async fn versions_in_use(pool: &PgPool) -> Result<BTreeMap<u32, u64>> {
    let rows: Vec<(Vec<u8>, i64)> = sqlx::query_as(
        "SELECT v, count(*) FROM (
             SELECT substring(totp_secret_enc FROM 1 FOR 4) AS v FROM yomi.users WHERE totp_secret_enc IS NOT NULL
             UNION ALL
             SELECT substring(totp_pending_secret_enc FROM 1 FOR 4) FROM yomi.users WHERE totp_pending_secret_enc IS NOT NULL
             UNION ALL
             SELECT substring(payload_enc FROM 1 FOR 4) FROM yomi.jobs WHERE payload_enc IS NOT NULL
             UNION ALL
             SELECT substring(secret_enc FROM 1 FOR 4) FROM yomi.webhooks
             UNION ALL
             SELECT substring(previous_secret_enc FROM 1 FOR 4) FROM yomi.webhooks WHERE previous_secret_enc IS NOT NULL
             UNION ALL
             SELECT substring(secret_enc FROM 1 FOR 4) FROM yomi.audit_sinks
             UNION ALL
             SELECT substring(api_key FROM 1 FOR 4) FROM yomi.workspace_llm
         ) s GROUP BY v",
    )
    .fetch_all(pool)
    .await?;
    let mut out = BTreeMap::new();
    for (v, n) in rows {
        if let Some(version) = version_of(&v) {
            *out.entry(version).or_default() += n as u64;
        }
    }
    Ok(out)
}

/// 現在の鍵（いちばん大きい版）以外で暗号化された値を、すべて現在の鍵で暗号化し直す。
/// 何度実行してもよい（現在の鍵のものは触らない）。
pub async fn reencrypt_all(pool: &PgPool, keys: &KeyRing) -> Result<Outcome> {
    let active = keys.active_version().to_be_bytes().to_vec();
    let mut outcome = Outcome::default();
    for column in ["totp_secret_enc", "totp_pending_secret_enc"] {
        reencrypt_users(pool, keys, &active, column, &mut outcome).await?;
    }
    reencrypt_jobs(pool, keys, &active, &mut outcome).await?;
    for column in ["secret_enc", "previous_secret_enc"] {
        reencrypt_webhooks(pool, keys, &active, column, &mut outcome).await?;
    }
    reencrypt_audit_sinks(pool, keys, &active, &mut outcome).await?;
    reencrypt_workspace_llm(pool, keys, &active, &mut outcome).await?;
    Ok(outcome)
}

async fn reencrypt_workspace_llm(
    pool: &PgPool,
    keys: &KeyRing,
    active: &[u8],
    outcome: &mut Outcome,
) -> Result<()> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT workspace_id FROM yomi.workspace_llm WHERE substring(api_key FROM 1 FOR 4) <> $1",
    )
    .bind(active)
    .fetch_all(pool)
    .await?;
    for chunk in ids.chunks(BATCH) {
        let mut tx = pool.begin().await?;
        let rows: Vec<(Uuid, Vec<u8>)> = sqlx::query_as(
            "SELECT workspace_id, api_key FROM yomi.workspace_llm
             WHERE workspace_id = ANY($1) AND substring(api_key FROM 1 FOR 4) <> $2
             FOR UPDATE",
        )
        .bind(chunk)
        .bind(active)
        .fetch_all(&mut *tx)
        .await?;
        for (ws, sealed) in rows {
            let aad = crate::workspace_llm::key_aad(ws);
            let Ok(plain) = keys.decrypt(&sealed, &aad) else {
                outcome.failed += 1;
                continue;
            };
            sqlx::query("UPDATE yomi.workspace_llm SET api_key = $2 WHERE workspace_id = $1")
                .bind(ws)
                .bind(keys.encrypt(&plain, &aad)?)
                .execute(&mut *tx)
                .await?;
            outcome.reencrypted += 1;
        }
        tx.commit().await?;
    }
    Ok(())
}

async fn reencrypt_audit_sinks(
    pool: &PgPool,
    keys: &KeyRing,
    active: &[u8],
    outcome: &mut Outcome,
) -> Result<()> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM yomi.audit_sinks WHERE substring(secret_enc FROM 1 FOR 4) <> $1",
    )
    .bind(active)
    .fetch_all(pool)
    .await?;
    for chunk in ids.chunks(BATCH) {
        let mut tx = pool.begin().await?;
        let rows: Vec<(Uuid, Vec<u8>)> = sqlx::query_as(
            "SELECT id, secret_enc FROM yomi.audit_sinks
             WHERE id = ANY($1) AND substring(secret_enc FROM 1 FOR 4) <> $2
             FOR UPDATE",
        )
        .bind(chunk)
        .bind(active)
        .fetch_all(&mut *tx)
        .await?;
        for (id, sealed) in rows {
            let aad = crate::audit_export::secret_aad(id);
            let Ok(plain) = keys.decrypt(&sealed, &aad) else {
                outcome.failed += 1;
                continue;
            };
            sqlx::query("UPDATE yomi.audit_sinks SET secret_enc = $2 WHERE id = $1")
                .bind(id)
                .bind(keys.encrypt(&plain, &aad)?)
                .execute(&mut *tx)
                .await?;
            outcome.reencrypted += 1;
        }
        tx.commit().await?;
    }
    Ok(())
}

async fn reencrypt_webhooks(
    pool: &PgPool,
    keys: &KeyRing,
    active: &[u8],
    column: &'static str,
    outcome: &mut Outcome,
) -> Result<()> {
    // 列の名前は上の固定の名前だけ。識別子は決められた関数（Ident）を通して埋め込む
    let column = crate::db::Ident::new(column)?;
    let ids: Vec<Uuid> = sqlx::query_scalar(crate::sql!(
        "SELECT id FROM yomi.webhooks WHERE {column} IS NOT NULL AND substring({column} FROM 1 FOR 4) <> $1"
    ))
    .bind(active)
    .fetch_all(pool)
    .await?;
    for chunk in ids.chunks(BATCH) {
        let mut tx = pool.begin().await?;
        let rows: Vec<(Uuid, Vec<u8>)> = sqlx::query_as(crate::sql!(
            "SELECT id, {column} FROM yomi.webhooks
             WHERE id = ANY($1) AND {column} IS NOT NULL AND substring({column} FROM 1 FOR 4) <> $2
             FOR UPDATE"
        ))
        .bind(chunk)
        .bind(active)
        .fetch_all(&mut *tx)
        .await?;
        for (id, sealed) in rows {
            let aad = crate::webhooks::secret_aad(id);
            let Ok(plain) = keys.decrypt(&sealed, &aad) else {
                outcome.failed += 1;
                continue;
            };
            sqlx::query(crate::sql!(
                "UPDATE yomi.webhooks SET {column} = $2 WHERE id = $1"
            ))
            .bind(id)
            .bind(keys.encrypt(&plain, &aad)?)
            .execute(&mut *tx)
            .await?;
            outcome.reencrypted += 1;
        }
        tx.commit().await?;
    }
    Ok(())
}

async fn reencrypt_users(
    pool: &PgPool,
    keys: &KeyRing,
    active: &[u8],
    column: &'static str,
    outcome: &mut Outcome,
) -> Result<()> {
    // 列の名前は上の固定の名前だけ。識別子は決められた関数（Ident）を通して埋め込む
    let column = crate::db::Ident::new(column)?;
    let ids: Vec<Uuid> = sqlx::query_scalar(crate::sql!(
        "SELECT id FROM yomi.users WHERE {column} IS NOT NULL AND substring({column} FROM 1 FOR 4) <> $1"
    ))
    .bind(active)
    .fetch_all(pool)
    .await?;
    for chunk in ids.chunks(BATCH) {
        let mut tx = pool.begin().await?;
        let rows: Vec<(Uuid, Vec<u8>)> = sqlx::query_as(crate::sql!(
            "SELECT id, {column} FROM yomi.users
             WHERE id = ANY($1) AND {column} IS NOT NULL AND substring({column} FROM 1 FOR 4) <> $2
             FOR UPDATE"
        ))
        .bind(chunk)
        .bind(active)
        .fetch_all(&mut *tx)
        .await?;
        for (id, sealed) in rows {
            let aad = crate::auth::mfa::aad(id);
            let Ok(plain) = keys.decrypt(&sealed, &aad) else {
                outcome.failed += 1;
                continue;
            };
            sqlx::query(crate::sql!(
                "UPDATE yomi.users SET {column} = $2 WHERE id = $1"
            ))
            .bind(id)
            .bind(keys.encrypt(&plain, &aad)?)
            .execute(&mut *tx)
            .await?;
            outcome.reencrypted += 1;
        }
        tx.commit().await?;
    }
    Ok(())
}

async fn reencrypt_jobs(
    pool: &PgPool,
    keys: &KeyRing,
    active: &[u8],
    outcome: &mut Outcome,
) -> Result<()> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM yomi.jobs WHERE payload_enc IS NOT NULL AND substring(payload_enc FROM 1 FOR 4) <> $1",
    )
    .bind(active)
    .fetch_all(pool)
    .await?;
    for chunk in ids.chunks(BATCH) {
        let mut tx = pool.begin().await?;
        // 実行中のジョブ（ほかのサーバーが取り出したもの）は飛ばす。次に実行したときに暗号化し直す
        let rows: Vec<(Uuid, String, Vec<u8>)> = sqlx::query_as(
            "SELECT id, kind, payload_enc FROM yomi.jobs
             WHERE id = ANY($1) AND payload_enc IS NOT NULL AND substring(payload_enc FROM 1 FOR 4) <> $2
             FOR UPDATE SKIP LOCKED",
        )
        .bind(chunk)
        .bind(active)
        .fetch_all(&mut *tx)
        .await?;
        for (id, kind, sealed) in rows {
            let aad = crate::jobs::aad(id, &kind);
            let Ok(plain) = keys.decrypt(&sealed, &aad) else {
                outcome.failed += 1;
                continue;
            };
            sqlx::query("UPDATE yomi.jobs SET payload_enc = $2 WHERE id = $1")
                .bind(id)
                .bind(keys.encrypt(&plain, &aad)?)
                .execute(&mut *tx)
                .await?;
            outcome.reencrypted += 1;
        }
        tx.commit().await?;
    }
    Ok(())
}
