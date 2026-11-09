//! 招待（Q104, Q121, Q193）。
//!
//! - 招待しても、本人が承諾するまで所属させない（INV-TEN-05）。
//! - トークンはランダムで一度だけ使え、短命。DBにはハッシュだけを保存する。
//! - リンクは設定の公開URLだけから作る（リクエストの Host ヘッダーは使わない）。
//! - 同じワークスペースの同じ宛先に招待し直すと、前の招待は使えなくなる。

use anyhow::Result;
use sqlx::{PgExecutor, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::access::Role;
use crate::auth::token;

/// 作った招待。`token` はこの場でだけ手に入る。
pub struct Created {
    pub id: Uuid,
    pub token: String,
    pub expires_at: String,
}

const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;

/// 招待を作る（同じ宛先への前の招待は失効させる）。
pub async fn create(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    email: &str,
    role: Role,
    invited_by: Uuid,
    lifetime_seconds: u32,
) -> Result<Created> {
    sqlx::query(
        "UPDATE yomi.invitations SET revoked_at = now()
         WHERE workspace_id = $1 AND email = $2 AND accepted_at IS NULL AND revoked_at IS NULL",
    )
    .bind(workspace_id)
    .bind(email)
    .execute(&mut **tx)
    .await?;
    let id = Uuid::new_v4();
    let token = token::generate();
    let expires_at: String = sqlx::query_scalar(crate::sql!(
        "INSERT INTO yomi.invitations (id, workspace_id, email, role, token_hash, invited_by, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, now() + make_interval(secs => $7))
         RETURNING to_char(expires_at AT TIME ZONE 'UTC', {RFC3339})"
    ))
    .bind(id)
    .bind(workspace_id)
    .bind(email)
    .bind(role.as_str())
    .bind(token::digest(&token))
    .bind(invited_by)
    .bind(f64::from(lifetime_seconds))
    .fetch_one(&mut **tx)
    .await?;
    Ok(Created {
        id,
        token,
        expires_at,
    })
}

/// 使える招待（未承諾・未失効・期限内）。
#[derive(Debug, Clone)]
pub struct Pending {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub email: String,
    pub role: Role,
}

/// トークンから、使える招待を引く。`lock` なら行をロックする（承諾のトランザクションの中で使う）。
pub async fn find_valid<'e>(
    executor: impl PgExecutor<'e>,
    presented: &str,
    lock: bool,
) -> Result<Option<Pending>> {
    let suffix = if lock { "FOR UPDATE" } else { "" };
    let row: Option<(Uuid, Uuid, String, String)> = sqlx::query_as(crate::sql!(
        "SELECT id, workspace_id, email, role FROM yomi.invitations
         WHERE token_hash = $1 AND accepted_at IS NULL AND revoked_at IS NULL AND expires_at > now()
         {suffix}"
    ))
    .bind(token::digest(presented))
    .fetch_optional(executor)
    .await?;
    Ok(row.and_then(|(id, workspace_id, email, role)| {
        Some(Pending {
            id,
            workspace_id,
            email,
            role: role.parse().ok()?,
        })
    }))
}

/// IDから、使える招待を引いてロックする（SSOでの承諾。トークンは始めたときに確かめてある）。
pub async fn find_valid_by_id_for_update(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Option<Pending>> {
    let row: Option<(Uuid, Uuid, String, String)> = sqlx::query_as(
        "SELECT id, workspace_id, email, role FROM yomi.invitations
         WHERE id = $1 AND accepted_at IS NULL AND revoked_at IS NULL AND expires_at > now()
         FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.and_then(|(id, workspace_id, email, role)| {
        Some(Pending {
            id,
            workspace_id,
            email,
            role: role.parse().ok()?,
        })
    }))
}

/// 承諾を記録し、所属させる。既に所属していれば、ロールは変えない。
pub async fn accept(
    tx: &mut Transaction<'_, Postgres>,
    invitation: &Pending,
    user_id: Uuid,
) -> Result<()> {
    sqlx::query("UPDATE yomi.invitations SET accepted_at = now() WHERE id = $1")
        .bind(invitation.id)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT INTO yomi.memberships (workspace_id, user_id, role) VALUES ($1, $2, $3)
         ON CONFLICT (workspace_id, user_id) DO NOTHING",
    )
    .bind(invitation.workspace_id)
    .bind(user_id)
    .bind(invitation.role.as_str())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// 一覧に出す、承諾待ちの招待。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub id: Uuid,
    pub email: String,
    pub role: String,
    pub created_at: String,
    pub expires_at: String,
}

pub async fn list_pending(pool: &PgPool, workspace_id: Uuid) -> Result<Vec<Summary>> {
    let rows: Vec<(Uuid, String, String, String, String)> = sqlx::query_as(crate::sql!(
        "SELECT id, email, role,
                to_char(created_at AT TIME ZONE 'UTC', {RFC3339}),
                to_char(expires_at AT TIME ZONE 'UTC', {RFC3339})
         FROM yomi.invitations
         WHERE workspace_id = $1 AND accepted_at IS NULL AND revoked_at IS NULL AND expires_at > now()
         ORDER BY created_at"
    ))
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, email, role, created_at, expires_at)| Summary {
            id,
            email,
            role,
            created_at,
            expires_at,
        })
        .collect())
}

/// 招待を取り消す。そのワークスペースの、承諾待ちの招待でなければ false。
pub async fn revoke(pool: &PgPool, workspace_id: Uuid, id: Uuid) -> Result<bool> {
    let done = sqlx::query(
        "UPDATE yomi.invitations SET revoked_at = now()
         WHERE id = $1 AND workspace_id = $2 AND accepted_at IS NULL AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(workspace_id)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// ある人が出した、ワークスペースの未使用の招待を失効させる（管理者でなくなったとき）。
pub async fn revoke_issued_by<'e>(
    executor: impl PgExecutor<'e>,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<()> {
    sqlx::query(
        "UPDATE yomi.invitations SET revoked_at = now()
         WHERE workspace_id = $1 AND invited_by = $2 AND accepted_at IS NULL AND revoked_at IS NULL",
    )
    .bind(workspace_id)
    .bind(user_id)
    .execute(executor)
    .await?;
    Ok(())
}

/// ある人が出した招待と、その人宛の招待を失効させる（所属から外したとき。戻って来られないように）。
pub async fn revoke_involving<'e>(
    executor: impl PgExecutor<'e>,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<()> {
    sqlx::query(
        "UPDATE yomi.invitations SET revoked_at = now()
         WHERE workspace_id = $1 AND accepted_at IS NULL AND revoked_at IS NULL
           AND (invited_by = $2 OR email = (SELECT email FROM yomi.users WHERE id = $2))",
    )
    .bind(workspace_id)
    .bind(user_id)
    .execute(executor)
    .await?;
    Ok(())
}

/// 招待のリンク。トークンはURLのフラグメントに入れ、サーバーやプロキシのアクセスログに残りにくくする。
pub fn link(public_url: &str, token: &str) -> String {
    format!(
        "{}/admin/invitations/accept#token={token}",
        public_url.trim_end_matches('/')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_use_the_public_url() {
        assert_eq!(
            link("https://yomi.example.com/", "abc"),
            "https://yomi.example.com/admin/invitations/accept#token=abc"
        );
    }
}
