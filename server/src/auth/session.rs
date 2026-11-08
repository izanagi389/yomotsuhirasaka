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
    /// 多要素認証（TOTPかパスキー）を設定しているか。
    pub mfa_enabled: bool,
    /// CMS側のパスキーを登録しているか。
    pub has_passkey: bool,
    /// 外部のOIDC/SSOでログインしたセッションか（個人トークンでは false）。
    pub sso: bool,
    /// サーバー管理者か、どこかのワークスペースの管理者か（照合した時点の状態）。
    pub admin_anywhere: bool,
    /// 本人が設定した言語（未設定なら None。Q198）。
    pub locale: Option<crate::locale::Locale>,
    /// 最後の本人確認（ログインか再認証）から、再認証が有効な時間内か。個人トークンでは常に false。
    pub reauth_fresh: bool,
    /// 再認証が有効な残りの秒数（有効でなければ0）。管理画面が、重い操作の前に再認証を求めるかを決めるのに使う。
    pub reauth_seconds_left: i64,
    /// 最後の本人確認から、再認証が有効な時間内か（方法は問わない）。
    authenticated_recently: bool,
    /// 個人トークンで認証したときの、トークンの範囲（Cookieのセッションなら None）。
    pub grant: Option<TokenGrant>,
    csrf_hash: Vec<u8>,
}

/// 個人トークンの範囲。
#[derive(Debug, Clone)]
pub struct TokenGrant {
    pub token_id: Uuid,
    pub scopes: Vec<String>,
    pub workspaces: Vec<Uuid>,
}

impl TokenGrant {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

impl Current {
    /// 管理操作に必要な多要素認証を満たしているか。
    /// SSOのセッションでは、多要素認証をIdPに任せるが、管理者にはCMS側のパスキーを求める（Q131）。
    pub fn admin_mfa_satisfied(&self) -> bool {
        if self.sso {
            self.has_passkey
        } else {
            self.mfa_enabled
        }
    }

    /// パスキーを登録してよいか。再認証が有効なら登録できる。
    /// パスキーのないSSOの管理者は、パスキーで再認証できないので、IdPでのログインの直後（再認証が有効な時間内）に限って登録できる（Q131）。
    pub fn may_enroll_passkey(&self) -> bool {
        self.reauth_fresh || (self.sso && !self.has_passkey && self.authenticated_recently)
    }

    pub fn csrf_matches(&self, presented: &str) -> bool {
        // 個人トークンにはCSRFトークンがない（Bearerの経路ではCSRFを確かめない）
        !self.csrf_hash.is_empty() && token::matches(presented, &self.csrf_hash)
    }

    /// 個人トークンで認証したときの立場を作る。`id` はトークンのID。
    pub fn from_token(
        user_id: Uuid,
        email: String,
        is_server_admin: bool,
        mfa_enabled: bool,
        locale: Option<crate::locale::Locale>,
        grant: TokenGrant,
    ) -> Self {
        Self {
            id: grant.token_id,
            user_id,
            email,
            is_server_admin,
            mfa_enabled,
            // 個人トークンでは、パスキーの有無とSSOは使わない（管理操作の判定は mfa_enabled で行う）
            has_passkey: false,
            sso: false,
            admin_anywhere: false,
            locale,
            reauth_fresh: false,
            reauth_seconds_left: 0,
            authenticated_recently: false,
            grant: Some(grant),
            csrf_hash: Vec::new(),
        }
    }

    /// 個人トークンで認証しているか。
    pub fn via_token(&self) -> bool {
        self.grant.is_some()
    }
}

/// 本人確認の方法（再認証の方法として記録する）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Password,
    Totp,
    Recovery,
    Passkey,
    Oidc,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::Totp => "totp",
            Self::Recovery => "recovery",
            Self::Passkey => "passkey",
            Self::Oidc => "oidc",
        }
    }
}

/// セッションを発行する（パスワードでのログイン）。
pub async fn issue(pool: &PgPool, user_id: Uuid, config: &SessionConfig) -> Result<Issued> {
    insert(
        pool,
        user_id,
        config.absolute_lifetime_seconds,
        false,
        false,
    )
    .await
}

/// SSOでログインしたセッションを発行する。
/// IdPからのリダイレクトで終わるので、CSRFトークンは、戻った画面が一度だけ受け取る（[`take_csrf_handoff`]）。
pub async fn issue_sso(pool: &PgPool, user_id: Uuid, config: &SessionConfig) -> Result<Issued> {
    insert(pool, user_id, config.absolute_lifetime_seconds, false, true).await
}

/// 多要素認証の途中のセッションを発行する。多要素認証のコードを送ること以外には使えない。
pub async fn issue_mfa_pending(pool: &PgPool, user_id: Uuid) -> Result<Issued> {
    insert(pool, user_id, MFA_PENDING_SECONDS, true, false).await
}

async fn insert(
    pool: &PgPool,
    user_id: Uuid,
    lifetime: u32,
    mfa_pending: bool,
    sso: bool,
) -> Result<Issued> {
    let issued = Issued {
        id: Uuid::new_v4(),
        token: token::generate(),
        csrf_token: token::generate(),
    };
    sqlx::query(
        "INSERT INTO yomi.sessions (id, user_id, token_hash, csrf_hash, expires_at, mfa_pending,
                                   auth_method, reauth_method, csrf_handoff)
         VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5), $6, $7, $8, $9)",
    )
    .bind(issued.id)
    .bind(user_id)
    .bind(token::digest(&issued.token))
    .bind(token::digest(&issued.csrf_token))
    .bind(f64::from(lifetime))
    .bind(mfa_pending)
    .bind(if sso { "oidc" } else { "password" })
    .bind(sso.then_some(Method::Oidc.as_str()))
    .bind(sso)
    .execute(pool)
    .await?;
    Ok(issued)
}

/// 多要素認証の途中のセッションを照合する。有効なら (セッションID, 利用者ID)。
pub async fn resolve_mfa_pending(pool: &PgPool, presented: &str) -> Result<Option<(Uuid, Uuid)>> {
    Ok(sqlx::query_as(
        "SELECT s.id, s.user_id
         FROM yomi.sessions s JOIN yomi.users u ON u.id = s.user_id
         WHERE s.token_hash = $1 AND s.mfa_pending
           AND s.revoked_at IS NULL AND s.expires_at > now() AND NOT u.disabled",
    )
    .bind(token::digest(presented))
    .fetch_optional(pool)
    .await?)
}

/// (id, user_id, email, is_server_admin, mfa_enabled, locale, csrf_hash, authenticated_recently, stale,
///  reauth_seconds_left, has_passkey, sso, reauth_method, admin_anywhere)
type ResolvedRow = (
    Uuid,
    Uuid,
    String,
    bool,
    bool,
    Option<String>,
    Vec<u8>,
    bool,
    bool,
    i64,
    bool,
    bool,
    Option<String>,
    bool,
);

/// Cookieで受け取ったセッションIDを照合する。無効なら None。
pub async fn resolve(
    pool: &PgPool,
    presented: &str,
    config: &SessionConfig,
) -> Result<Option<Current>> {
    let row: Option<ResolvedRow> = sqlx::query_as(crate::sql!(
        "SELECT s.id, s.user_id, u.email, u.is_server_admin, {}, u.locale, s.csrf_hash,
                s.authenticated_at > now() - make_interval(secs => $3),
                s.last_seen_at < now() - make_interval(secs => $4),
                GREATEST(0, floor(extract(epoch FROM s.authenticated_at + make_interval(secs => $3) - now())))::bigint,
                EXISTS (SELECT 1 FROM yomi.passkeys pk WHERE pk.user_id = u.id),
                s.auth_method = 'oidc',
                s.reauth_method,
                u.is_server_admin OR EXISTS (SELECT 1 FROM yomi.memberships m WHERE m.user_id = u.id AND m.role = 'admin')
         FROM yomi.sessions s JOIN yomi.users u ON u.id = s.user_id
         WHERE s.token_hash = $1
           AND NOT s.mfa_pending
           AND s.revoked_at IS NULL
           AND s.expires_at > now()
           AND s.last_seen_at > now() - make_interval(secs => $2)
           AND NOT u.disabled",
        crate::auth::mfa::MFA_ENABLED_U
    ))
    .bind(token::digest(presented))
    .bind(f64::from(config.idle_timeout_seconds))
    .bind(f64::from(config.reauth_window_seconds))
    .bind(TOUCH_INTERVAL_SECONDS)
    .fetch_optional(pool)
    .await?;
    let Some((
        id,
        user_id,
        email,
        is_server_admin,
        mfa_enabled,
        locale,
        csrf_hash,
        authenticated_recently,
        stale,
        reauth_seconds_left,
        has_passkey,
        sso,
        reauth_method,
        admin_anywhere,
    )) = row
    else {
        return Ok(None);
    };
    let reauth_fresh =
        authenticated_recently && sso_reauth_counts(sso, admin_anywhere, reauth_method.as_deref());
    if stale {
        sqlx::query("UPDATE yomi.sessions SET last_seen_at = now() WHERE id = $1")
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
        has_passkey,
        sso,
        admin_anywhere,
        locale: locale.and_then(|l| l.parse().ok()),
        reauth_fresh,
        reauth_seconds_left: if reauth_fresh { reauth_seconds_left } else { 0 },
        authenticated_recently,
        grant: None,
        csrf_hash,
    }))
}

/// 最後の本人確認が、再認証として有効か（時間とは別の条件）。
/// SSOでログインした管理者は、CMS側のパスキーでの本人確認だけを有効とする（Q131）。
/// 管理者でなければ、IdPでの再ログインも有効（Q108）。
fn sso_reauth_counts(sso: bool, admin_anywhere: bool, reauth_method: Option<&str>) -> bool {
    !sso || !admin_anywhere || reauth_method == Some(Method::Passkey.as_str())
}

/// 本人確認をした時刻を、今にする（再認証）。`method` は本人確認の方法。
pub async fn mark_reauthenticated(pool: &PgPool, session_id: Uuid, method: Method) -> Result<()> {
    sqlx::query(
        "UPDATE yomi.sessions SET authenticated_at = now(), reauth_method = $2
         WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(session_id)
    .bind(method.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

/// SSOでログインしたセッションの、CSRFトークンを一度だけ発行する（戻った画面が受け取る）。
/// 既に受け取っていれば None。
pub async fn take_csrf_handoff(pool: &PgPool, session_id: Uuid) -> Result<Option<String>> {
    let csrf = token::generate();
    let done = sqlx::query(
        "UPDATE yomi.sessions SET csrf_hash = $2, csrf_handoff = false
         WHERE id = $1 AND csrf_handoff AND revoked_at IS NULL AND expires_at > now()",
    )
    .bind(session_id)
    .bind(token::digest(&csrf))
    .execute(pool)
    .await?;
    Ok((done.rows_affected() == 1).then_some(csrf))
}

/// セッションが有効か（失効・期限切れでない）。
pub async fn is_active(pool: &PgPool, session_id: Uuid, user_id: Uuid) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM yomi.sessions WHERE id = $1 AND user_id = $2
                        AND NOT mfa_pending AND revoked_at IS NULL AND expires_at > now())",
    )
    .bind(session_id)
    .bind(user_id)
    .fetch_one(pool)
    .await?)
}

/// セッションを失効させる。
pub async fn revoke(pool: &PgPool, session_id: Uuid) -> Result<()> {
    sqlx::query("UPDATE yomi.sessions SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Cookieで受け取ったセッションIDのセッションを失効させる（ログインの前に持っていたセッションなど）。
pub async fn revoke_presented(pool: &PgPool, presented: &str) -> Result<()> {
    sqlx::query(
        "UPDATE yomi.sessions SET revoked_at = now() WHERE token_hash = $1 AND revoked_at IS NULL",
    )
    .bind(token::digest(presented))
    .execute(pool)
    .await?;
    Ok(())
}

/// 本人のセッションを1つ失効させる。本人のものでなければ false。
pub async fn revoke_own(pool: &PgPool, user_id: Uuid, session_id: Uuid) -> Result<bool> {
    let done = sqlx::query(
        "UPDATE yomi.sessions SET revoked_at = now()
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
    sqlx::query("UPDATE yomi.sessions SET revoked_at = now() WHERE user_id = $1 AND id <> $2 AND revoked_at IS NULL")
        .bind(user_id)
        .bind(keep)
        .execute(executor)
        .await?;
    Ok(())
}

/// 利用者のセッションをすべて失効させる（リセットの完了、緊急停止など）。
pub async fn revoke_all<'e>(executor: impl PgExecutor<'e>, user_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE yomi.sessions SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
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
         FROM yomi.sessions
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sso_admins_reauthenticate_only_with_passkeys() {
        // パスワードでログインしたセッションは、方法を問わない（方法の制限は再認証の処理で行う）
        assert!(sso_reauth_counts(false, true, None));
        // SSOで、管理者でなければ IdP での再ログインも有効
        assert!(sso_reauth_counts(true, false, Some("oidc")));
        // SSOの管理者は、パスキーだけ
        assert!(!sso_reauth_counts(true, true, Some("oidc")));
        assert!(!sso_reauth_counts(true, true, None));
        assert!(sso_reauth_counts(true, true, Some("passkey")));
    }
}
