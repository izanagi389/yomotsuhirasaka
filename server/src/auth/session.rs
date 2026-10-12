//! 管理画面のセッション（Q91, Q107, Q108）。
//!
//! - セッションIDとCSRFトークンは、発行時に一度だけ渡し、DBにはハッシュだけを保存する。
//! - 無操作の期限と絶対的な期限の両方を設ける。時刻の比較はDBの時計で行う。
//! - 利用者が無効にされたら、セッションは直ちに使えなくなる（照合のたびに利用者の状態を見る）。
//! - 権限（ロールや所属）はセッションに保存しない。必要なときに、その時点の状態と照合する。

use anyhow::Result;
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use super::token;
use crate::config::SessionConfig;

/// 無操作の期限の判定に使う、最終利用時刻の更新の間隔（秒）。毎回は書き込まない。
const TOUCH_INTERVAL_SECONDS: f64 = 30.0;
/// パスワードを通った後、多要素認証のコードを送るまでの期限（秒）。
pub const MFA_PENDING_SECONDS: u32 = 5 * 60;

/// 新しく発行したセッション。`token` と `csrf_token` はこの場でだけ手に入る。
pub struct Issued {
    pub id: Uuid,
    pub token: String,
    pub csrf_token: String,
}

/// 照合できた、有効なセッション。
#[derive(Debug, Clone)]
pub struct Current {
    pub id: Uuid,
    pub user_id: Uuid,
    pub email: String,
    pub is_server_admin: bool,
    /// 多要素認証（TOTP）を設定しているか。
    pub mfa_enabled: bool,
    /// 最後の本人確認（ログインか再認証）から、再認証が有効な時間内か。
    pub reauth_fresh: bool,
    csrf_hash: Vec<u8>,
}

impl Current {
    pub fn csrf_matches(&self, presented: &str) -> bool {
        token::matches(presented, &self.csrf_hash)
    }
}

/// セッションを発行する。
pub async fn issue(pool: &PgPool, user_id: Uuid, config: &SessionConfig) -> Result<Issued> {
    insert(pool, user_id, config.absolute_lifetime_seconds, false).await
}

/// 多要素認証の途中のセッションを発行する。多要素認証のコードを送ること以外には使えない。
pub async fn issue_mfa_pending(pool: &PgPool, user_id: Uuid) -> Result<Issued> {
    insert(pool, user_id, MFA_PENDING_SECONDS, true).await
}

async fn insert(pool: &PgPool, user_id: Uuid, lifetime: u32, mfa_pending: bool) -> Result<Issued> {
    let issued = Issued {
        id: Uuid::new_v4(),
        token: token::generate(),
        csrf_token: token::generate(),
    };
    sqlx::query(
        "INSERT INTO cms.sessions (id, user_id, token_hash, csrf_hash, expires_at, mfa_pending)
         VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5), $6)",
    )
    .bind(issued.id)
    .bind(user_id)
    .bind(token::digest(&issued.token))
    .bind(token::digest(&issued.csrf_token))
    .bind(f64::from(lifetime))
    .bind(mfa_pending)
    .execute(pool)
    .await?;
    Ok(issued)
}

/// 多要素認証の途中のセッションを照合する。有効なら (セッションID, 利用者ID)。
pub async fn resolve_mfa_pending(pool: &PgPool, presented: &str) -> Result<Option<(Uuid, Uuid)>> {
    Ok(sqlx::query_as(
        "SELECT s.id, s.user_id
         FROM cms.sessions s JOIN cms.users u ON u.id = s.user_id
         WHERE s.token_hash = $1 AND s.mfa_pending
           AND s.revoked_at IS NULL AND s.expires_at > now() AND NOT u.disabled",
    )
    .bind(token::digest(presented))
    .fetch_optional(pool)
    .await?)
}

/// (id, user_id, email, is_server_admin, mfa_enabled, csrf_hash, reauth_fresh, stale)
type ResolvedRow = (Uuid, Uuid, String, bool, bool, Vec<u8>, bool, bool);

/// Cookieで受け取ったセッションIDを照合する。無効なら None。
pub async fn resolve(
    pool: &PgPool,
    presented: &str,
    config: &SessionConfig,
) -> Result<Option<Current>> {
    let row: Option<ResolvedRow> = sqlx::query_as(
        "SELECT s.id, s.user_id, u.email, u.is_server_admin, u.totp_enabled_at IS NOT NULL, s.csrf_hash,
                s.authenticated_at > now() - make_interval(secs => $3),
                s.last_seen_at < now() - make_interval(secs => $4)
         FROM cms.sessions s JOIN cms.users u ON u.id = s.user_id
         WHERE s.token_hash = $1
           AND NOT s.mfa_pending
           AND s.revoked_at IS NULL
           AND s.expires_at > now()
           AND s.last_seen_at > now() - make_interval(secs => $2)
           AND NOT u.disabled",
    )
    .bind(token::digest(presented))
    .bind(f64::from(config.idle_timeout_seconds))
    .bind(f64::from(config.reauth_window_seconds))
    .bind(TOUCH_INTERVAL_SECONDS)
    .fetch_optional(pool)
    .await?;
    let Some((id, user_id, email, is_server_admin, mfa_enabled, csrf_hash, reauth_fresh, stale)) =
        row
    else {
        return Ok(None);
    };
    if stale {
        sqlx::query("UPDATE cms.sessions SET last_seen_at = now() WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
    }
    Ok(Some(Current {
        id,
        user_id,
        email,
        is_server_admin,
        mfa_enabled,
        reauth_fresh,
        csrf_hash,
    }))
}

/// 本人確認をした時刻を、今にする（再認証）。
pub async fn mark_reauthenticated(pool: &PgPool, session_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE cms.sessions SET authenticated_at = now() WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(session_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// セッションを失効させる。
pub async fn revoke(pool: &PgPool, session_id: Uuid) -> Result<()> {
    sqlx::query("UPDATE cms.sessions SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Cookieで受け取ったセッションIDのセッションを失効させる（ログインの前に持っていたセッションなど）。
pub async fn revoke_presented(pool: &PgPool, presented: &str) -> Result<()> {
    sqlx::query(
        "UPDATE cms.sessions SET revoked_at = now() WHERE token_hash = $1 AND revoked_at IS NULL",
    )
    .bind(token::digest(presented))
    .execute(pool)
    .await?;
    Ok(())
}

/// 本人のセッションを1つ失効させる。本人のものでなければ false。
pub async fn revoke_own(pool: &PgPool, user_id: Uuid, session_id: Uuid) -> Result<bool> {
    let done = sqlx::query(
        "UPDATE cms.sessions SET revoked_at = now()
         WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL AND expires_at > now()",
    )
    .bind(session_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// 利用者の、指定したもの以外のセッションをすべて失効させる（パスワードや多要素認証の変更。Q107）。
pub async fn revoke_others<'e>(
    executor: impl PgExecutor<'e>,
    user_id: Uuid,
    keep: Uuid,
) -> Result<()> {
    sqlx::query("UPDATE cms.sessions SET revoked_at = now() WHERE user_id = $1 AND id <> $2 AND revoked_at IS NULL")
        .bind(user_id)
        .bind(keep)
        .execute(executor)
        .await?;
    Ok(())
}

/// 本人のログイン中のセッションの一覧に出す情報。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub id: Uuid,
    pub created_at: String,
    pub last_seen_at: String,
    pub expires_at: String,
    pub current: bool,
}

/// 本人の有効なセッションの一覧（新しい順）。
pub async fn list_active(
    pool: &PgPool,
    user_id: Uuid,
    current: Uuid,
    config: &SessionConfig,
) -> Result<Vec<Summary>> {
    const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;
    let sql = crate::sql!(
        "SELECT id,
                to_char(created_at AT TIME ZONE 'UTC', {RFC3339}),
                to_char(last_seen_at AT TIME ZONE 'UTC', {RFC3339}),
                to_char(expires_at AT TIME ZONE 'UTC', {RFC3339})
         FROM cms.sessions
         WHERE user_id = $1 AND NOT mfa_pending AND revoked_at IS NULL AND expires_at > now()
           AND last_seen_at > now() - make_interval(secs => $2)
         ORDER BY created_at DESC"
    );
    let rows: Vec<(Uuid, String, String, String)> = sqlx::query_as(sql)
        .bind(user_id)
        .bind(f64::from(config.idle_timeout_seconds))
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(id, created_at, last_seen_at, expires_at)| Summary {
            id,
            created_at,
            last_seen_at,
            expires_at,
            current: id == current,
        })
        .collect())
}
