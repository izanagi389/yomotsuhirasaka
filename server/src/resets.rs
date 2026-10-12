//! パスワードと多要素認証のリセット（Q100, Q104, Q117）。
//!
//! - トークンはランダムで一度だけ使え、短命。DBにはハッシュだけを保存する。
//! - リンクは設定の公開URLだけから作る（リクエストの Host ヘッダーは使わない）。
//! - サーバー管理者のトークンは、どの経路で作られても使えない（INV-AUTH-10）。サーバー管理者の回復はサーバー上のCLIで行う。
//! - 同じ利用者・同じ種類のトークンを新しく作ると、前のものは使えなくなる。

use anyhow::Result;
use sqlx::{PgExecutor, Postgres, Transaction};
use uuid::Uuid;

use crate::auth::token;

/// リセットの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Password,
    Mfa,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Password => "password",
            Kind::Mfa => "mfa",
        }
    }

    fn path(self) -> &'static str {
        match self {
            Kind::Password => "reset-password",
            Kind::Mfa => "reset-mfa",
        }
    }
}

const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;

/// 作ったトークン。`token` はこの場でだけ手に入る。
pub struct Issued {
    pub id: Uuid,
    pub token: String,
    pub expires_at: String,
}

/// トークンを作る（同じ利用者・同じ種類の前のトークンは失効させる）。
pub async fn issue(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    kind: Kind,
    issued_by: Option<Uuid>,
    lifetime_seconds: u32,
) -> Result<Issued> {
    sqlx::query(
        "UPDATE cms.reset_tokens SET revoked_at = now()
         WHERE user_id = $1 AND kind = $2 AND used_at IS NULL AND revoked_at IS NULL",
    )
    .bind(user_id)
    .bind(kind.as_str())
    .execute(&mut **tx)
    .await?;
    let id = Uuid::new_v4();
    let token = token::generate();
    let expires_at: String = sqlx::query_scalar(crate::sql!(
        "INSERT INTO cms.reset_tokens (id, user_id, kind, token_hash, issued_by, expires_at)
         VALUES ($1, $2, $3, $4, $5, now() + make_interval(secs => $6))
         RETURNING to_char(expires_at AT TIME ZONE 'UTC', {RFC3339})"
    ))
    .bind(id)
    .bind(user_id)
    .bind(kind.as_str())
    .bind(token::digest(&token))
    .bind(issued_by)
    .bind(f64::from(lifetime_seconds))
    .fetch_one(&mut **tx)
    .await?;
    Ok(Issued {
        id,
        token,
        expires_at,
    })
}

/// 本人の要求で、直前（1分以内）に同じ種類のトークンを作っていれば true（メールの連打を防ぐ）。
pub async fn recently_issued<'e>(
    executor: impl PgExecutor<'e>,
    user_id: Uuid,
    kind: Kind,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM cms.reset_tokens
                        WHERE user_id = $1 AND kind = $2 AND used_at IS NULL AND revoked_at IS NULL
                          AND created_at > now() - interval '1 minute')",
    )
    .bind(user_id)
    .bind(kind.as_str())
    .fetch_one(executor)
    .await?)
}

/// 使えるトークン。
#[derive(Debug, Clone)]
pub struct Valid {
    pub id: Uuid,
    pub user_id: Uuid,
    pub email: String,
    pub locale: Option<String>,
}

/// トークンから、使えるトークンを引く（未使用・未失効・期限内、利用者が無効でもサーバー管理者でもない）。
/// `lock` なら行をロックする（完了のトランザクションの中で使う）。
pub async fn find_valid<'e>(
    executor: impl PgExecutor<'e>,
    presented: &str,
    kind: Kind,
    lock: bool,
) -> Result<Option<Valid>> {
    let suffix = if lock { "FOR UPDATE OF r" } else { "" };
    let row: Option<(Uuid, Uuid, String, Option<String>)> = sqlx::query_as(crate::sql!(
        "SELECT r.id, r.user_id, u.email, u.locale
         FROM cms.reset_tokens r JOIN cms.users u ON u.id = r.user_id
         WHERE r.token_hash = $1 AND r.kind = $2
           AND r.used_at IS NULL AND r.revoked_at IS NULL AND r.expires_at > now()
           AND NOT u.disabled AND NOT u.is_server_admin
         {suffix}"
    ))
    .bind(token::digest(presented))
    .bind(kind.as_str())
    .fetch_optional(executor)
    .await?;
    Ok(row.map(|(id, user_id, email, locale)| Valid {
        id,
        user_id,
        email,
        locale,
    }))
}

/// トークンを使用済みにする。
pub async fn mark_used(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> Result<()> {
    sqlx::query("UPDATE cms.reset_tokens SET used_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// リセットのリンク。トークンはURLのフラグメントに入れる（アクセスログに残りにくくする）。
pub fn link(public_url: &str, kind: Kind, token: &str) -> String {
    format!(
        "{}/admin/{}#token={token}",
        public_url.trim_end_matches('/'),
        kind.path()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links() {
        assert_eq!(
            link("https://cms.example.com/", Kind::Password, "t"),
            "https://cms.example.com/admin/reset-password#token=t"
        );
        assert_eq!(
            link("https://cms.example.com", Kind::Mfa, "t"),
            "https://cms.example.com/admin/reset-mfa#token=t"
        );
    }
}
