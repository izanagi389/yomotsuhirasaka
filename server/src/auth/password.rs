//! パスワードの保管と照合（Argon2id）と、パスワードの強さの規則（Q170）。
//!
//! - ハッシュは argon2 クレートの既定の設定（Argon2id、19 MiB、2回、並列度1。OWASPの推奨値）で作る。
//! - 照合はCPUを重く使うので、非同期の処理を止めないよう、別のスレッドで行う。
//! - 登録されていないメールアドレスでも、ダミーのハッシュで照合を行い、応答時間の差を小さくする（Q189）。

use std::sync::LazyLock;

use anyhow::{Result, anyhow};
use argon2::Argon2;
use argon2::password_hash::{PasswordHasher as _, PasswordVerifier as _};

/// パスワードの最短の長さ（文字数）。
pub const MIN_LENGTH: usize = 12;
/// パスワードの最長の長さ（文字数）。極端に長い入力で照合を重くさせないため。
pub const MAX_LENGTH: usize = 256;

/// 新しいパスワードが規則に合わない理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weakness {
    TooShort,
    TooLong,
}

impl Weakness {
    /// APIのエラーメッセージ（英語）。
    pub fn message(self) -> String {
        match self {
            Weakness::TooShort => format!("The password must be at least {MIN_LENGTH} characters."),
            Weakness::TooLong => format!("The password must be at most {MAX_LENGTH} characters."),
        }
    }
}

/// 新しいパスワードの強さの検査。
///
/// TODO(Q178): 同梱のよく使われるパスワードの一覧との照合は、一覧の出典の承認を受けてから加える。
pub fn check_policy(password: &str) -> Result<(), Weakness> {
    let len = password.chars().count();
    if len < MIN_LENGTH {
        return Err(Weakness::TooShort);
    }
    if len > MAX_LENGTH {
        return Err(Weakness::TooLong);
    }
    Ok(())
}

/// パスワードのハッシュ（PHC文字列）を作る。別のスレッドで実行する。
pub async fn hash(password: String) -> Result<String> {
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|h| h.to_string())
            .map_err(|e| anyhow!("cannot hash password: {e}"))
    })
    .await?
}

/// 登録されていない利用者の照合に使う、ダミーのハッシュ（プロセスの起動後に一度だけ作る）。
static DUMMY_HASH: LazyLock<String> = LazyLock::new(|| {
    let mut random = [0u8; 32];
    rand::fill(&mut random);
    Argon2::default()
        .hash_password(&random)
        .map(|h| h.to_string())
        .expect("hashing random bytes with default parameters cannot fail")
});

/// パスワードを照合する。`stored` が None（パスワードのない利用者・存在しない利用者）でも、
/// ダミーのハッシュで同じだけの計算をしてから false を返す。
pub async fn verify(password: String, stored: Option<String>) -> Result<bool> {
    tokio::task::spawn_blocking(move || {
        let (hash, real) = match stored {
            Some(h) => (h, true),
            None => (DUMMY_HASH.clone(), false),
        };
        let ok = Argon2::default()
            .verify_password(password.as_bytes(), hash.as_str())
            .is_ok();
        ok && real
    })
    .await
    .map_err(Into::into)
}

/// 起動時にダミーのハッシュを作っておく（最初のログインだけ遅くならないように）。
pub fn warm_up() {
    LazyLock::force(&DUMMY_HASH);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hash_and_verify() {
        let h = hash("correct horse battery".to_owned()).await.unwrap();
        assert!(h.starts_with("$argon2id$"), "{h}");
        assert!(
            verify("correct horse battery".to_owned(), Some(h.clone()))
                .await
                .unwrap()
        );
        assert!(
            !verify("wrong horse battery".to_owned(), Some(h))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn missing_hash_never_matches() {
        assert!(!verify(String::new(), None).await.unwrap());
        assert!(!verify("anything at all".to_owned(), None).await.unwrap());
    }

    #[tokio::test]
    async fn malformed_hash_never_matches() {
        assert!(
            !verify("x".repeat(20), Some("not-a-hash".to_owned()))
                .await
                .unwrap()
        );
    }

    #[test]
    fn policy() {
        assert_eq!(check_policy("Abc!2345678"), Err(Weakness::TooShort)); // 11文字
        assert_eq!(check_policy("Abc!23456789"), Ok(())); // 12文字
        assert_eq!(check_policy("パスワードはこれで十二文字です"), Ok(())); // 文字数で数える
        assert_eq!(check_policy(&"a1".repeat(129)), Err(Weakness::TooLong));
    }
}
