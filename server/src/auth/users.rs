//! 利用者（インスタンス単位。Q101）の読み書き。

use anyhow::{Result, bail};
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

/// メールアドレスを比較用の形（前後の空白を除き、小文字）にする。明らかに形が違うものは None。
pub fn normalize_email(raw: &str) -> Option<String> {
    let email = raw.trim().to_lowercase();
    let (local, domain) = email.rsplit_once('@')?;
    let ok = !local.is_empty()
        && !domain.is_empty()
        && email.len() <= 254
        && !email.chars().any(|c| c.is_whitespace() || c.is_control());
    ok.then_some(email)
}

/// 表示名の長さの上限（文字数）。
pub const MAX_DISPLAY_NAME_CHARS: usize = 50;

/// 表示名を、保存する形（前後の空白を除く）にする。空・長すぎる・制御文字を含むものは None。
pub fn normalize_display_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    let ok = !name.is_empty()
        && name.chars().count() <= MAX_DISPLAY_NAME_CHARS
        && !name.chars().any(char::is_control);
    ok.then(|| name.to_owned())
}

/// ログインの照合に使う情報。
pub struct LoginRecord {
    pub id: Uuid,
    pub password_hash: Option<String>,
    pub disabled: bool,
    /// ログインの失敗による待ち時間中か。
    pub blocked: bool,
    /// 多要素認証（TOTPかパスキー）を設定しているか。
    pub mfa_enabled: bool,
}

/// (id, password_hash, disabled, blocked, mfa_enabled)
type LoginRow = (Uuid, Option<String>, bool, bool, bool);

/// 多要素認証の有無は [`super::mfa::MFA_ENABLED_U`]（TOTPかパスキー）で判定する。
const LOGIN_COLUMNS: &str =
    "u.id, u.password_hash, u.disabled, coalesce(u.login_blocked_until > now(), false)";

fn login_record((id, password_hash, disabled, blocked, mfa_enabled): LoginRow) -> LoginRecord {
    LoginRecord {
        id,
        password_hash,
        disabled,
        blocked,
        mfa_enabled,
    }
}

pub async fn find_for_login(pool: &PgPool, email: &str) -> Result<Option<LoginRecord>> {
    let row: Option<LoginRow> = sqlx::query_as(crate::sql!(
        "SELECT {LOGIN_COLUMNS}, {} FROM yomi.users u WHERE u.email = $1",
        super::mfa::MFA_ENABLED_U
    ))
    .bind(email)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(login_record))
}

pub async fn find_by_id_for_login(pool: &PgPool, id: Uuid) -> Result<Option<LoginRecord>> {
    let row: Option<LoginRow> = sqlx::query_as(crate::sql!(
        "SELECT {LOGIN_COLUMNS}, {} FROM yomi.users u WHERE u.id = $1",
        super::mfa::MFA_ENABLED_U
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(login_record))
}

/// 利用者を作る。同じメールアドレスの利用者がいればエラー。
pub async fn create(
    pool: &PgPool,
    email: &str,
    password_hash: &str,
    is_server_admin: bool,
) -> Result<Uuid> {
    let id = Uuid::new_v4();
    let inserted = sqlx::query(
        "INSERT INTO yomi.users (id, email, password_hash, is_server_admin, password_changed_at)
         VALUES ($1, $2, $3, $4, now())
         ON CONFLICT (email) DO NOTHING",
    )
    .bind(id)
    .bind(email)
    .bind(password_hash)
    .bind(is_server_admin)
    .execute(pool)
    .await?;
    if inserted.rows_affected() == 0 {
        bail!("a user with this email already exists");
    }
    Ok(id)
}

/// パスワードを変える。失敗の記録も消す。
pub async fn set_password<'e>(
    executor: impl PgExecutor<'e>,
    id: Uuid,
    password_hash: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE yomi.users SET password_hash = $2, password_changed_at = now(),
                failed_login_count = 0, login_blocked_until = NULL
         WHERE id = $1",
    )
    .bind(id)
    .bind(password_hash)
    .execute(executor)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_are_trimmed_and_checked() {
        assert_eq!(
            normalize_display_name("  山田 花子 ").as_deref(),
            Some("山田 花子")
        );
        assert_eq!(
            normalize_display_name(&"あ".repeat(50)).map(|n| n.chars().count()),
            Some(50)
        );
        for bad in ["", "   ", "a\nb", "a\u{7}b"] {
            assert_eq!(normalize_display_name(bad), None, "{bad:?}");
        }
        assert_eq!(normalize_display_name(&"あ".repeat(51)), None);
    }

    #[test]
    fn emails() {
        assert_eq!(
            normalize_email("  Alice@Example.COM "),
            Some("alice@example.com".to_owned())
        );
        assert_eq!(normalize_email("a@b"), Some("a@b".to_owned()));
        for bad in [
            "",
            "alice",
            "@example.com",
            "alice@",
            "a b@example.com",
            "a@b\u{0}",
        ] {
            assert_eq!(normalize_email(bad), None, "{bad:?}");
        }
    }
}
