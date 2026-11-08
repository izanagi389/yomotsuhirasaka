//! プレビュー用トークン（03-content-and-editing.md「プレビュー（Q72）」、計画の13番、INV-PRV-01, 02）。
//! ワークスペースのロールに切り替えたトランザクションの中で呼ぶ。
//!
//! - 形は `yomi_prv_<ランダムな43文字>`。発行のときに一度だけ渡し、DBにはハッシュだけを残す（auth/token.rs）。
//! - 1つの記事の下書きだけを読める。配信APIでは、読み取り用のAPIキーと一緒に専用のヘッダーで送ったときだけ効く
//!   （トークンだけでは配信APIを使えない。INV-PRV-02）。
//! - 有効期間は最長10分（既定も10分）。失効させたもの・期限を過ぎたものは、照合のたびに無いものとして扱う（原則3）。

use anyhow::Result;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::token;

/// トークンの接頭辞（URLのクエリに入っていたら断るため。INV-AUTH-11）。
pub const PREFIX: &str = "yomi_prv_";
/// 有効期間の既定と上限（秒）。
pub const MAX_TTL_SECONDS: i64 = 600;

const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;

pub struct Issued {
    pub id: Uuid,
    pub token: String,
    pub expires_at: String,
}

/// 記事のトークンを発行する（記事がない・削除済みなら None）。同じ記事の、期限を1日以上過ぎた行は消す。
pub async fn issue(
    conn: &mut PgConnection,
    entry_id: Uuid,
    created_by: Uuid,
    ttl_seconds: i64,
) -> Result<Option<Issued>> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM entries WHERE id = $1 AND deleted_at IS NULL)",
    )
    .bind(entry_id)
    .fetch_one(&mut *conn)
    .await?;
    if !exists {
        return Ok(None);
    }
    sqlx::query(
        "DELETE FROM preview_tokens WHERE entry_id = $1 AND expires_at < now() - interval '1 day'",
    )
    .bind(entry_id)
    .execute(&mut *conn)
    .await?;
    let token = format!("{PREFIX}{}", token::generate());
    let id = Uuid::new_v4();
    let expires_at: String = sqlx::query_scalar(crate::sql!(
        "INSERT INTO preview_tokens (id, entry_id, token_hash, created_by, expires_at)
         VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5))
         RETURNING to_char(expires_at AT TIME ZONE 'UTC', {RFC3339})"
    ))
    .bind(id)
    .bind(entry_id)
    .bind(token::digest(&token))
    .bind(created_by)
    .bind(ttl_seconds.clamp(1, MAX_TTL_SECONDS) as f64)
    .fetch_one(conn)
    .await?;
    Ok(Some(Issued {
        id,
        token,
        expires_at,
    }))
}

/// トークンを失効させる（その記事のトークンでなければ false）。
pub async fn revoke(conn: &mut PgConnection, entry_id: Uuid, token_id: Uuid) -> Result<bool> {
    Ok(sqlx::query(
        "UPDATE preview_tokens SET revoked_at = COALESCE(revoked_at, now()) WHERE id = $1 AND entry_id = $2",
    )
    .bind(token_id)
    .bind(entry_id)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// 有効なトークンの対象の記事と、有効期限（形が違う・失効・期限切れ・記事が削除済みなら None）。
pub async fn lookup(conn: &mut PgConnection, presented: &str) -> Result<Option<(Uuid, String)>> {
    if !presented.starts_with(PREFIX) {
        return Ok(None);
    }
    Ok(sqlx::query_as(crate::sql!(
        "SELECT t.entry_id, to_char(t.expires_at AT TIME ZONE 'UTC', {RFC3339})
         FROM preview_tokens t JOIN entries e ON e.id = t.entry_id
         WHERE t.token_hash = $1 AND t.revoked_at IS NULL AND t.expires_at > now() AND e.deleted_at IS NULL"
    ))
    .bind(token::digest(presented))
    .fetch_optional(conn)
    .await?)
}
