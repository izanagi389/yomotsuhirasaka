//! 緊急停止の操作（13-security.md「緊急停止の操作」、Q171, Q180）。
//!
//! - サーバー上のCLI（`yomi-server` のサブコマンド）からだけ呼ぶ。ネットワーク越しの管理APIには出さない（INV-AUTH-23）。
//! - 実行は監査ログに記録する（操作した人は記録しない。サーバーの上で実行できる人の操作として扱う）。

use anyhow::Result;
use sqlx::PgPool;
use uuid::Uuid;

use crate::audit;
use crate::auth::personal_tokens::API_KEY_PREFIX as PREFIX;
use crate::auth::token;
use crate::workspace_db;

/// 一斉失効の結果（件数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Revoked {
    pub sessions: u64,
    pub personal_tokens: u64,
    pub reset_tokens: u64,
    pub invitations: u64,
    pub preview_tokens: u64,
}

/// すべてのセッション（多要素認証の途中のものを含む）、個人トークン、リセットのトークン、未承諾の招待、
/// すべてのワークスペースのプレビュー用トークンを失効させる。出回っているかもしれないものを、まとめて止める。
///
/// 共通のスキーマのものは1つのトランザクションで失効させ、記録する。プレビュー用トークンはワークスペースごとの
/// トランザクションで失効させる。途中で失敗しても、もう一度実行すれば残りを失効させられる。
pub async fn revoke_all_sessions_and_tokens(pool: &PgPool) -> Result<Revoked> {
    let mut tx = pool.begin().await?;
    let sessions =
        sqlx::query("UPDATE yomi.sessions SET revoked_at = now() WHERE revoked_at IS NULL")
            .execute(&mut *tx)
            .await?
            .rows_affected();
    let personal_tokens =
        sqlx::query("UPDATE yomi.personal_tokens SET revoked_at = now() WHERE revoked_at IS NULL")
            .execute(&mut *tx)
            .await?
            .rows_affected();
    let reset_tokens = sqlx::query(
        "UPDATE yomi.reset_tokens SET revoked_at = now() WHERE used_at IS NULL AND revoked_at IS NULL",
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let invitations = sqlx::query(
        "UPDATE yomi.invitations SET revoked_at = now() WHERE accepted_at IS NULL AND revoked_at IS NULL",
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    audit::record(
        &mut *tx,
        None,
        None,
        audit::action::EMERGENCY_SESSIONS_REVOKED,
        &[],
    )
    .await?;
    tx.commit().await?;

    let workspaces: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM yomi.workspaces ORDER BY created_at")
            .fetch_all(pool)
            .await?;
    let mut preview_tokens = 0;
    for id in workspaces {
        let Some(db) = workspace_db::lookup(pool, id).await? else {
            continue;
        };
        let mut tx = workspace_db::begin(pool, &db).await?;
        preview_tokens += sqlx::query(
            "UPDATE preview_tokens SET revoked_at = now() WHERE revoked_at IS NULL AND expires_at > now()",
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
    }
    Ok(Revoked {
        sessions,
        personal_tokens,
        reset_tokens,
        invitations,
        preview_tokens,
    })
}

/// 入れ替えた配信APIのキー。`secret` はこの場でだけ手に入る。
#[derive(Debug, Clone)]
pub struct RotatedKey {
    pub workspace_id: Uuid,
    pub key_id: Uuid,
    pub name: String,
    pub secret: String,
}

/// 有効な配信APIのキーの秘密の値を、すべて新しくする（`workspace` を指定すればそのワークスペースだけ）。
/// キーのID・名前・範囲・用途・有効期限は変えない。古い値は直ちに使えなくなる。
pub async fn rotate_api_keys(pool: &PgPool, workspace: Option<Uuid>) -> Result<Vec<RotatedKey>> {
    let mut tx = pool.begin().await?;
    let keys: Vec<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT id, workspace_id, name FROM yomi.api_keys
         WHERE revoked_at IS NULL AND (expires_at IS NULL OR expires_at > now())
           AND ($1::uuid IS NULL OR workspace_id = $1)
         ORDER BY workspace_id, created_at
         FOR UPDATE",
    )
    .bind(workspace)
    .fetch_all(&mut *tx)
    .await?;
    let mut rotated = Vec::with_capacity(keys.len());
    for (key_id, workspace_id, name) in keys {
        let secret = format!("{PREFIX}{}", token::generate());
        let display_prefix: String = secret.chars().take(PREFIX.len() + 4).collect();
        sqlx::query(
            "UPDATE yomi.api_keys SET key_hash = $2, display_prefix = $3, last_used_at = NULL WHERE id = $1",
        )
        .bind(key_id)
        .bind(token::digest(&secret))
        .bind(&display_prefix)
        .execute(&mut *tx)
        .await?;
        rotated.push(RotatedKey {
            workspace_id,
            key_id,
            name,
            secret,
        });
    }
    // ワークスペースごとに、入れ替えたキーのIDを記録する
    let mut by_workspace: Vec<(Uuid, Vec<Uuid>)> = Vec::new();
    for k in &rotated {
        match by_workspace.last_mut() {
            Some((ws, ids)) if *ws == k.workspace_id => ids.push(k.key_id),
            _ => by_workspace.push((k.workspace_id, vec![k.key_id])),
        }
    }
    for (ws, ids) in by_workspace {
        audit::record(
            &mut *tx,
            Some(ws),
            None,
            audit::action::API_KEYS_ROTATED,
            &ids,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(rotated)
}
