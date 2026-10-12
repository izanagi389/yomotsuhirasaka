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
    /// 同じ文字の繰り返し、連続した文字や数字、キーボードの並びでできている。
    SimplePattern,
    /// メールアドレスやサイト名を含む。
    ContainsPersonalInfo,
}

impl Weakness {
    /// APIのエラーメッセージ（英語）。
    pub fn message(self) -> String {
        match self {
            Weakness::TooShort => format!("The password must be at least {MIN_LENGTH} characters."),
            Weakness::TooLong => format!("The password must be at most {MAX_LENGTH} characters."),
            Weakness::SimplePattern => "The password is a simple pattern (repeated characters, a sequence or a keyboard row). Choose a less predictable one.".to_owned(),
            Weakness::ContainsPersonalInfo => "The password must not contain your email address or the site name.".to_owned(),
        }
    }
}

/// 新しいパスワードの強さの検査（Q170, Q196）。
///
/// - 12文字以上、256文字以下。記号の混在などは求めない。
/// - 単純なパターン（同じ文字の繰り返し、連続した文字や数字、キーボードの並び）が大半を占めるものは拒否する。
/// - `personal` に渡した語（メールアドレス、その @ より前、サイトのホスト名など。3文字以上のもの）を含むものは拒否する。
///
/// よく使われるパスワードの一覧は同梱しない（Q196）。
pub fn check_policy(password: &str, personal: &[&str]) -> Result<(), Weakness> {
    let len = password.chars().count();
    if len < MIN_LENGTH {
        return Err(Weakness::TooShort);
    }
    if len > MAX_LENGTH {
        return Err(Weakness::TooLong);
    }
    let lower = password.to_lowercase();
    if personal
        .iter()
        .map(|w| w.trim().to_lowercase())
        .any(|w| w.chars().count() >= 3 && lower.contains(&w))
    {
        return Err(Weakness::ContainsPersonalInfo);
    }
    if pattern_coverage(&lower) * 10 >= len * 8 {
        return Err(Weakness::SimplePattern);
    }
    Ok(())
}

/// メールアドレスと公開URLから、パスワードに含めてはいけない語を作る。
pub fn personal_words(email: &str, public_url: &str) -> Vec<String> {
    let mut words = vec![email.to_owned()];
    if let Some((local, _)) = email.split_once('@') {
        words.push(local.to_owned());
    }
    let host = public_url
        .split_once("://")
        .map_or(public_url, |(_, r)| r)
        .split(['/', ':', '?', '#'])
        .next()
        .unwrap_or("");
    words.push(host.to_owned());
    // サイト名らしい部分（www などのよくある接頭辞と、最後のドメインの部分を除いた、4文字以上のラベル）
    let labels: Vec<&str> = host.split('.').collect();
    for label in labels.iter().take(labels.len().saturating_sub(1)) {
        if label.len() >= 4 && !matches!(*label, "www" | "blog" | "admin") {
            words.push((*label).to_owned());
        }
    }
    words
}

/// キーボードの横の並び（QWERTY）。
const KEYBOARD_ROWS: [&str; 4] = ["1234567890-=", "qwertyuiop[]", "asdfghjkl;'", "zxcvbnm,./"];

/// キーの位置（行, 横の位置）。行ごとに半キーずつずれている配列を、そのまま座標にする。
fn key_position(c: char) -> Option<(i32, i32)> {
    KEYBOARD_ROWS.iter().enumerate().find_map(|(row, line)| {
        let col = line.chars().position(|k| k == c)?;
        let row = i32::try_from(row).ok()?;
        let col = i32::try_from(col).ok()?;
        // 横の位置は半キー単位（行が1つ下がるごとに右へ半キー）
        Some((row, col * 2 + row))
    })
}

/// 2文字が「続けて打ちやすい」並びか（同じ文字、文字や数字の前後、キーボードの上下左右と斜めの隣）。
fn adjacent(a: char, b: char) -> bool {
    if a == b {
        return true;
    }
    let same_class = (a.is_ascii_lowercase() && b.is_ascii_lowercase())
        || (a.is_ascii_digit() && b.is_ascii_digit());
    if same_class && ((a as i32 - b as i32).abs() == 1 || matches!((a, b), ('9', '0') | ('0', '9')))
    {
        return true;
    }
    match (key_position(a), key_position(b)) {
        // 同じ行なら1キー（2半キー）、隣の行なら左右1.5キー（3半キー）まで
        (Some((ra, xa)), Some((rb, xb))) => match (ra - rb).abs() {
            0 => (xa - xb).abs() == 2,
            1 => (xa - xb).abs() <= 3,
            _ => false,
        },
        _ => false,
    }
}

/// 「続けて打ちやすい」並びが3文字以上続く部分の文字数の合計。
fn pattern_coverage(lower: &str) -> usize {
    let chars: Vec<char> = lower.chars().collect();
    let mut covered = 0;
    let mut start = 0;
    for i in 1..=chars.len() {
        if i == chars.len() || !adjacent(chars[i - 1], chars[i]) {
            if i - start >= 3 {
                covered += i - start;
            }
            start = i;
        }
    }
    covered
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
        assert_eq!(check_policy("Abc!2345678", &[]), Err(Weakness::TooShort)); // 11文字
        assert_eq!(check_policy("Tq8!vmz2Lp0w", &[]), Ok(())); // 12文字
        assert_eq!(check_policy("パスワードはこれで十二文字です", &[]), Ok(())); // 文字数で数える
        assert_eq!(check_policy(&"a1".repeat(129), &[]), Err(Weakness::TooLong));
    }

    #[test]
    fn simple_patterns_are_rejected() {
        for p in [
            "aaaaaaaaaaaa",
            "AAAAAAAAAAAAAA",
            "123456789012",
            "abcdefghijkl",
            "zyxwvutsrqpo",
            "qwertyuiop12",
            "1qaz2wsx3edc",
            "asdfghjkl;'z",
            "111122223333",
            "q1w2e3r4t5y6", // 横の並びと縦の隣を交互に打つ形
        ] {
            assert_eq!(check_policy(p, &[]), Err(Weakness::SimplePattern), "{p}");
        }
    }

    #[test]
    fn ordinary_passwords_pass() {
        for p in [
            "correct horse battery staple",
            "a long enough passphrase",
            "another long passphrase",
            "Tq8!vmz2Lp0w",
            "smoke-test-password-123",
            "inv-3f9c2a71-5b0e-4c1d-9e2f-8a7b6c5d4e3f",
            "猫と珈琲と雨の日の午後の読書",
        ] {
            assert_eq!(check_policy(p, &[]), Ok(()), "{p}");
        }
    }

    #[test]
    fn random_passwords_are_not_mistaken_for_patterns() {
        // ランダムな12文字（英数字）が単純なパターンと見なされることは、ほとんどない（0.5%未満）。
        // 拒否されても、別のパスワードを選び直せばよい
        const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let mut rejected = 0;
        for _ in 0..5000 {
            let mut bytes = [0u8; 12];
            rand::fill(&mut bytes);
            let p: String = bytes
                .iter()
                .map(|b| char::from(ALPHABET[usize::from(*b) % ALPHABET.len()]))
                .collect();
            if check_policy(&p, &[]).is_err() {
                rejected += 1;
            }
        }
        assert!(
            rejected < 25,
            "{rejected} of 5000 random passwords were rejected"
        );
    }

    #[test]
    fn personal_information_is_rejected() {
        let words = personal_words("Alice.Smith@example.com", "https://myblog.example.com/");
        let refs: Vec<&str> = words.iter().map(String::as_str).collect();
        for p in [
            "alice.smith#2026",
            "my-ALICE.SMITH-pw",
            "love myblog forever",
            "myblog.example.com!",
        ] {
            assert_eq!(
                check_policy(p, &refs),
                Err(Weakness::ContainsPersonalInfo),
                "{p}"
            );
        }
        assert_eq!(check_policy("an unrelated passphrase", &refs), Ok(()));
        // 短い語やよくある部分（example、com）は対象にしない
        assert!(!words.iter().any(|w| w == "com"));
    }
}
