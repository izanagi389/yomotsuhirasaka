//! CMSが発行して照合するランダムな値（セッションID、CSRFトークンなど）。
//!
//! 発行した値は利用者に一度だけ渡し、DBにはSHA-256のハッシュだけを保存する（13-security.md「鍵」）。
//! 値は256ビットのランダムなので、ハッシュに塩やストレッチングは要らない。

use base64::Engine as _;
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;

/// 新しいランダムな値（256ビット、URLで安全なBase64、43文字）。
pub fn generate() -> String {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// DBに保存するハッシュ。
pub fn digest(value: &str) -> Vec<u8> {
    Sha256::digest(value.as_bytes()).to_vec()
}

/// 受け取った値が、保存したハッシュと一致するか（比較の時間は内容によらない）。
pub fn matches(value: &str, stored_digest: &[u8]) -> bool {
    digest(value).ct_eq(stored_digest).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_values_are_unique_and_url_safe() {
        let a = generate();
        let b = generate();
        assert_ne!(a, b);
        assert_eq!(a.len(), 43);
        assert!(
            a.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }

    #[test]
    fn matching() {
        let v = generate();
        let d = digest(&v);
        assert!(matches(&v, &d));
        assert!(!matches(&generate(), &d));
        assert!(!matches(&v, &d[..31]));
        assert!(!matches("", &d));
    }
}
