//! 多要素認証: TOTP（認証アプリのワンタイムコード）と復旧コード（Q90, Q107, Q108）。
//!
//! - TOTPは RFC 6238 の標準の設定（HMAC-SHA1、6桁、30秒、前後1区切りまで許す）。計算は totp-rs クレート。
//! - TOTPの秘密は暗号化の鍵で暗号化して保存する。AADに利用者のIDを入れ、他の利用者の行に移しても使えないようにする。
//! - 同じ時間の区切りのコードは二度受け付けない（RFC 6238 5.2）。
//! - 復旧コードは10個。80ビットのランダムな値で、一度だけ使える。DBにはハッシュだけを保存する。

use anyhow::{Context, Result, anyhow};
use sqlx::{PgExecutor, PgPool, Postgres, Transaction};
use totp_rs::{Builder, Secret, Totp};
use uuid::Uuid;

use super::token;
use crate::keys::KeyRing;

/// TOTPの秘密の長さ（160ビット。RFC 4226 の推奨）。
pub const SECRET_BYTES: usize = 20;
/// 一度に発行する復旧コードの数。
pub const RECOVERY_CODE_COUNT: usize = 10;
/// 復旧コードの文字（Crockford の Base32。I・L・O・U を使わない32文字）。1文字5ビット、16文字で80ビット。
/// 入力では、I と L は 1、O は 0 と読む。
const RECOVERY_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// 多要素認証を設定しているか（TOTPか、パスキーが1つ以上）のSQLの式。`yomi.users` を `u` と名付けた問い合わせで使う。
pub const MFA_ENABLED_U: &str = "(u.totp_enabled_at IS NOT NULL OR EXISTS (SELECT 1 FROM yomi.passkeys pk WHERE pk.user_id = u.id))";
/// 同じ式を、`yomi.users` に別名を付けない問い合わせで使う形。
pub const MFA_ENABLED: &str = "(yomi.users.totp_enabled_at IS NOT NULL OR EXISTS (SELECT 1 FROM yomi.passkeys pk WHERE pk.user_id = yomi.users.id))";

pub(crate) fn aad(user_id: Uuid) -> Vec<u8> {
    format!("yomi:totp:{user_id}").into_bytes()
}

fn totp_from(secret: Vec<u8>) -> Result<Totp> {
    Builder::new()
        .with_secret(secret)
        .build()
        .map_err(|e| anyhow!("invalid TOTP configuration: {e:?}"))
}

/// 受け取ったコードを、空白を除いた形にする。
fn clean_code(code: &str) -> String {
    code.chars().filter(|c| !c.is_whitespace()).collect()
}

/// 設定の途中で、認証アプリに登録してもらう情報。
pub struct Enrollment {
    pub secret_base32: String,
    pub otpauth_url: String,
}

/// URLの値に使えない文字を、%XX の形にする（RFC 3986 の unreserved 以外）。
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// 設定を始める: 新しい秘密を作り、確認前の秘密として保存する（前の途中の設定は捨てる）。
pub async fn begin_enrollment(
    pool: &PgPool,
    keys: &KeyRing,
    user_id: Uuid,
    account: &str,
    issuer: &str,
) -> Result<Enrollment> {
    let mut bytes = [0u8; SECRET_BYTES];
    rand::fill(&mut bytes);
    let sealed = keys.encrypt(&bytes, &aad(user_id))?;
    sqlx::query("UPDATE yomi.users SET totp_pending_secret_enc = $2 WHERE id = $1")
        .bind(user_id)
        .bind(sealed)
        .execute(pool)
        .await?;
    let secret_base32 = Secret::from(bytes).to_base32();
    let label = percent_encode(&format!("{issuer}:{account}"));
    let otpauth_url = format!(
        "otpauth://totp/{label}?secret={secret_base32}&issuer={}&algorithm=SHA1&digits=6&period=30",
        percent_encode(issuer)
    );
    Ok(Enrollment {
        secret_base32,
        otpauth_url,
    })
}

/// 暗号化された秘密でコードを確かめ、合えばその時間の区切りを返す。
fn check_code(keys: &KeyRing, user_id: Uuid, sealed: &[u8], code: &str) -> Result<Option<u64>> {
    let secret = keys
        .decrypt(sealed, &aad(user_id))
        .context("cannot decrypt TOTP secret")?;
    Ok(totp_from(secret)?.check_current(&clean_code(code)))
}

/// 設定を確定する: 確認前の秘密でコードが合えば有効にし、復旧コードを発行して返す。
/// 既にほかの方法（パスキー）で多要素認証を設定していれば、復旧コードは作り直さない（空を返す）。
/// コードが合わない・途中の設定がなければ None。
/// 呼ぶ側のトランザクションで確定する（セッションの失効・通知・監査ログと同じトランザクションにするため）。
/// `None` のときは何も変えていない。
pub async fn confirm_enrollment(
    tx: &mut Transaction<'_, Postgres>,
    keys: &KeyRing,
    user_id: Uuid,
    code: &str,
) -> Result<Option<Vec<String>>> {
    let pending: Option<Option<Vec<u8>>> =
        sqlx::query_scalar("SELECT totp_pending_secret_enc FROM yomi.users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&mut **tx)
            .await?;
    let Some(sealed) = pending.flatten() else {
        return Ok(None);
    };
    let Some(step) = check_code(keys, user_id, &sealed, code)? else {
        return Ok(None);
    };
    let had_mfa: bool = sqlx::query_scalar(crate::sql!(
        "SELECT {MFA_ENABLED_U} FROM yomi.users u WHERE u.id = $1 FOR UPDATE"
    ))
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?;
    let done = sqlx::query(
        "UPDATE yomi.users SET totp_secret_enc = totp_pending_secret_enc, totp_pending_secret_enc = NULL,
                totp_enabled_at = now(), totp_last_step = $3
         WHERE id = $1 AND totp_pending_secret_enc = $2",
    )
    .bind(user_id)
    .bind(&sealed)
    .bind(step_i64(step))
    .execute(&mut **tx)
    .await?;
    if done.rows_affected() != 1 {
        // 同時に別の設定が始まった
        return Ok(None);
    }
    let codes = if had_mfa {
        Vec::new()
    } else {
        replace_recovery_codes(tx, user_id).await?
    };
    Ok(Some(codes))
}

fn step_i64(step: u64) -> i64 {
    i64::try_from(step).unwrap_or(i64::MAX)
}

/// 多要素認証をすべて止める（TOTP・パスキー・復旧コード）。多要素認証のリセットで使う。
pub async fn disable(tx: &mut Transaction<'_, Postgres>, user_id: Uuid) -> Result<()> {
    clear_totp(tx, user_id).await?;
    sqlx::query("DELETE FROM yomi.passkeys WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM yomi.recovery_codes WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn clear_totp(tx: &mut Transaction<'_, Postgres>, user_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE yomi.users SET totp_secret_enc = NULL, totp_pending_secret_enc = NULL,
                totp_enabled_at = NULL, totp_last_step = NULL
         WHERE id = $1",
    )
    .bind(user_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// TOTPだけを止める。ほかの方法（パスキー）が残っていなければ、復旧コードも消す。
/// 多要素認証が残っていれば true。
pub async fn remove_totp(tx: &mut Transaction<'_, Postgres>, user_id: Uuid) -> Result<bool> {
    sqlx::query("SELECT 1 FROM yomi.users WHERE id = $1 FOR UPDATE")
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    clear_totp(tx, user_id).await?;
    forget_recovery_codes_if_no_method(tx, user_id).await
}

/// 多要素認証の方法が1つも残っていなければ、復旧コードを消す。方法が残っていれば true。
pub async fn forget_recovery_codes_if_no_method(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<bool> {
    let still: bool = sqlx::query_scalar(crate::sql!(
        "SELECT {MFA_ENABLED_U} FROM yomi.users u WHERE u.id = $1"
    ))
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?;
    if !still {
        sqlx::query("DELETE FROM yomi.recovery_codes WHERE user_id = $1")
            .bind(user_id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(still)
}

/// TOTPを設定しているか。
pub async fn totp_enabled(pool: &PgPool, user_id: Uuid) -> Result<bool> {
    Ok(
        sqlx::query_scalar("SELECT totp_enabled_at IS NOT NULL FROM yomi.users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await?
            .unwrap_or(false),
    )
}

/// 設定済みのTOTPでコードを確かめる。合えば、その時間の区切りを使用済みにする（同じコードは二度通らない）。
pub async fn verify_totp(pool: &PgPool, keys: &KeyRing, user_id: Uuid, code: &str) -> Result<bool> {
    let sealed: Option<Option<Vec<u8>>> =
        sqlx::query_scalar("SELECT totp_secret_enc FROM yomi.users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    let Some(sealed) = sealed.flatten() else {
        return Ok(false);
    };
    let Some(step) = check_code(keys, user_id, &sealed, code)? else {
        return Ok(false);
    };
    let done = sqlx::query(
        "UPDATE yomi.users SET totp_last_step = $2
         WHERE id = $1 AND totp_secret_enc IS NOT NULL
           AND (totp_last_step IS NULL OR totp_last_step < $2)",
    )
    .bind(user_id)
    .bind(step_i64(step))
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// 復旧コードの比較用の形（大文字、ハイフンと空白を除き、読み違えやすい文字をそろえる）。
fn normalize_recovery_code(code: &str) -> String {
    code.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| match c.to_ascii_uppercase() {
            'I' | 'L' => '1',
            'O' => '0',
            other => other,
        })
        .collect()
}

/// 新しい復旧コード（`XXXX-XXXX-XXXX-XXXX`）。
pub fn generate_recovery_code() -> String {
    let mut bytes = [0u8; 16];
    rand::fill(&mut bytes);
    let chars: Vec<char> = bytes
        .iter()
        .map(|b| char::from(RECOVERY_ALPHABET[usize::from(b & 31)]))
        .collect();
    chars
        .chunks(4)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-")
}

/// 復旧コードを使う。未使用のコードと一致すれば、使用済みにして true。
pub async fn use_recovery_code(pool: &PgPool, user_id: Uuid, code: &str) -> Result<bool> {
    let normalized = normalize_recovery_code(code);
    if normalized.is_empty() {
        return Ok(false);
    }
    let done = sqlx::query(
        "UPDATE yomi.recovery_codes SET used_at = now()
         WHERE user_id = $1 AND code_hash = $2 AND used_at IS NULL",
    )
    .bind(user_id)
    .bind(token::digest(&normalized))
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// 復旧コードを作り直す（古いコードはすべて使えなくなる）。発行したコードを返す（この場でだけ手に入る）。
pub async fn replace_recovery_codes(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<Vec<String>> {
    sqlx::query("DELETE FROM yomi.recovery_codes WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    let codes: Vec<String> = (0..RECOVERY_CODE_COUNT)
        .map(|_| generate_recovery_code())
        .collect();
    for code in &codes {
        insert_recovery_code(&mut **tx, user_id, code).await?;
    }
    Ok(codes)
}

async fn insert_recovery_code<'e>(
    executor: impl PgExecutor<'e>,
    user_id: Uuid,
    code: &str,
) -> Result<()> {
    sqlx::query("INSERT INTO yomi.recovery_codes (id, user_id, code_hash) VALUES ($1, $2, $3)")
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(token::digest(&normalize_recovery_code(code)))
        .execute(executor)
        .await?;
    Ok(())
}

/// 未使用の復旧コードの数。
pub async fn remaining_recovery_codes(pool: &PgPool, user_id: Uuid) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM yomi.recovery_codes WHERE user_id = $1 AND used_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_codes_have_80_bits_and_normalize() {
        let code = generate_recovery_code();
        assert_eq!(code.len(), 19, "{code}");
        assert_eq!(code.matches('-').count(), 3);
        let normalized = normalize_recovery_code(&code.to_lowercase().replace('-', " "));
        assert_eq!(normalized, code.replace('-', ""));
        assert_ne!(generate_recovery_code(), code);
        assert_eq!(normalize_recovery_code("ol-Il"), "0111");
        // 32文字がすべて異なる（1文字5ビット）
        let mut alphabet = RECOVERY_ALPHABET.to_vec();
        alphabet.sort_unstable();
        alphabet.dedup();
        assert_eq!(alphabet.len(), 32);
    }

    #[test]
    fn totp_matches_rfc6238_sha1_vector() {
        // RFC 6238 付録B の試験値（秘密 "12345678901234567890"、8桁のうち下6桁）
        let totp = totp_from(b"12345678901234567890".to_vec()).unwrap();
        assert_eq!(totp.check("287082", 59), Some(1));
        assert_eq!(totp.check("287082", 59 + 30), Some(1)); // 前の区切りまで許す
        assert_eq!(totp.check("287082", 59 + 90), None);
        assert_eq!(totp.check(" 287082 ".trim(), 59), Some(1));
        assert_eq!(totp.check("+87082", 59), None);
    }

    #[test]
    fn url_encoding() {
        assert_eq!(
            percent_encode("yomi.example.com:a+b@x.test"),
            "yomi.example.com%3Aa%2Bb%40x.test"
        );
    }
}
