//! 暗号化の鍵のファイル。
//!
//! 1行に1つの鍵を `v<バージョン>:<32バイトのBase64>` の形で書く。`#` で始まる行は注釈。
//! いちばん大きいバージョンを、新しく暗号化するときに使う（鍵の入れ替え、Q93）。
//! 鍵がない・読めない・形が正しくない場合、サーバーは起動しない（INV-KEY-02）。
//!
//! 暗号化は XChaCha20-Poly1305（chacha20poly1305 クレート）。暗号文の形は
//! `[鍵のバージョン 4バイト（ビッグエンディアン）][ノンス 24バイト][暗号文とタグ]`。
//! ノンスは毎回ランダムに作る（24バイトなので衝突を気にしなくてよい）。
//! 追加の認証データ（AAD）に、値の持ち主と用途を入れ、暗号文を別の行や別の用途に移しても復号できないようにする。

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use chacha20poly1305::aead::{Aead as _, KeyInit as _, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

pub const KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 24;
const HEADER_BYTES: usize = 4 + NONCE_BYTES;

/// 鍵の一覧（バージョン → 鍵）。値は表示しない。
pub struct KeyRing {
    keys: BTreeMap<u32, [u8; KEY_BYTES]>,
}

impl std::fmt::Debug for KeyRing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyRing")
            .field("versions", &self.keys.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl KeyRing {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read encryption key file {}", path.display()))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mut keys = BTreeMap::new();
        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((version, encoded)) = line.split_once(':') else {
                bail!(
                    "encryption key file line {}: expected `v<version>:<base64>`",
                    i + 1
                );
            };
            let version: u32 = version
                .strip_prefix('v')
                .and_then(|v| v.parse().ok())
                .with_context(|| format!("encryption key file line {}: invalid version", i + 1))?;
            let bytes = STANDARD
                .decode(encoded.trim())
                .with_context(|| format!("encryption key file line {}: invalid base64", i + 1))?;
            let key: [u8; KEY_BYTES] = bytes.try_into().map_err(|_| {
                anyhow::anyhow!(
                    "encryption key file line {}: key must be {KEY_BYTES} bytes",
                    i + 1
                )
            })?;
            if keys.insert(version, key).is_some() {
                bail!("encryption key file: version v{version} appears twice");
            }
        }
        if keys.is_empty() {
            bail!("encryption key file contains no keys");
        }
        Ok(Self { keys })
    }

    /// 新しく暗号化するときに使う鍵のバージョン。
    pub fn active_version(&self) -> u32 {
        *self
            .keys
            .keys()
            .next_back()
            .expect("key ring is never empty")
    }

    fn cipher(&self, version: u32) -> Result<XChaCha20Poly1305> {
        let key = self
            .keys
            .get(&version)
            .ok_or_else(|| anyhow!("encryption key v{version} is not in the key file"))?;
        XChaCha20Poly1305::new_from_slice(key).map_err(|_| anyhow!("invalid key length"))
    }

    /// 現在の鍵で暗号化する。
    pub fn encrypt(&self, plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
        let version = self.active_version();
        let mut nonce = [0u8; NONCE_BYTES];
        rand::fill(&mut nonce);
        let sealed = self
            .cipher(version)?
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| anyhow!("encryption failed"))?;
        let mut out = Vec::with_capacity(HEADER_BYTES + sealed.len());
        out.extend_from_slice(&version.to_be_bytes());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    /// 暗号文に記録された版の鍵で復号する。改ざん・AADの不一致・鍵がないときはエラー。
    pub fn decrypt(&self, blob: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
        if blob.len() < HEADER_BYTES {
            bail!("ciphertext is too short");
        }
        let version = u32::from_be_bytes(blob[..4].try_into().expect("4 bytes"));
        let nonce: [u8; NONCE_BYTES] = blob[4..HEADER_BYTES].try_into().expect("24 bytes");
        self.cipher(version)?
            .decrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: &blob[HEADER_BYTES..],
                    aad,
                },
            )
            .map_err(|_| anyhow!("decryption failed (wrong key, tampered data or wrong context)"))
    }
}

/// 新しい鍵のファイルの中身（バージョン1の鍵を1つ）。
pub fn generate_key_file() -> String {
    let mut key = [0u8; KEY_BYTES];
    rand::fill(&mut key);
    format!(
        "# CMS encryption keys. Keep this file separate from database backups.\n# Losing it makes stored secrets unreadable.\nv1:{}\n",
        STANDARD.encode(key)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_file_parses() {
        let ring = KeyRing::parse(&generate_key_file()).unwrap();
        assert_eq!(ring.active_version(), 1);
    }

    #[test]
    fn highest_version_is_active() {
        let k = STANDARD.encode([7u8; KEY_BYTES]);
        let ring = KeyRing::parse(&format!("v1:{k}\nv3:{k}\nv2:{k}\n")).unwrap();
        assert_eq!(ring.active_version(), 3);
    }

    #[test]
    fn rejects_bad_files() {
        let short = STANDARD.encode([1u8; 16]);
        for bad in [
            "",
            "# only comments\n",
            "v1:not-base64!!",
            &format!("v1:{short}"),
            "1:AAAA",
            "vx:AAAA",
        ] {
            assert!(KeyRing::parse(bad).is_err(), "should reject {bad:?}");
        }
        let k = STANDARD.encode([1u8; KEY_BYTES]);
        assert!(KeyRing::parse(&format!("v1:{k}\nv1:{k}")).is_err());
    }

    #[test]
    fn encrypt_and_decrypt() {
        let ring = KeyRing::parse(&generate_key_file()).unwrap();
        let blob = ring.encrypt(b"secret", b"ctx:1").unwrap();
        assert_eq!(ring.decrypt(&blob, b"ctx:1").unwrap(), b"secret");
        assert!(!blob.windows(6).any(|w| w == b"secret"));
        // ノンスは毎回違う
        assert_ne!(blob, ring.encrypt(b"secret", b"ctx:1").unwrap());
        // 別の文脈・改ざん・別の鍵では復号できない
        assert!(ring.decrypt(&blob, b"ctx:2").is_err());
        let mut tampered = blob.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(ring.decrypt(&tampered, b"ctx:1").is_err());
        let other = KeyRing::parse(&generate_key_file()).unwrap();
        assert!(other.decrypt(&blob, b"ctx:1").is_err());
        assert!(ring.decrypt(&blob[..10], b"ctx:1").is_err());
    }

    #[test]
    fn old_versions_still_decrypt_after_rotation() {
        let k1 = STANDARD.encode([1u8; KEY_BYTES]);
        let k2 = STANDARD.encode([2u8; KEY_BYTES]);
        let old = KeyRing::parse(&format!("v1:{k1}")).unwrap();
        let blob = old.encrypt(b"x", b"a").unwrap();
        let rotated = KeyRing::parse(&format!("v1:{k1}\nv2:{k2}")).unwrap();
        assert_eq!(rotated.decrypt(&blob, b"a").unwrap(), b"x");
        assert_eq!(
            &rotated.encrypt(b"x", b"a").unwrap()[..4],
            &2u32.to_be_bytes()
        );
        // 古い鍵を外すと、古い暗号文は読めない
        let removed = KeyRing::parse(&format!("v2:{k2}")).unwrap();
        assert!(removed.decrypt(&blob, b"a").is_err());
    }

    #[test]
    fn debug_does_not_print_keys() {
        let k = STANDARD.encode([9u8; KEY_BYTES]);
        let ring = KeyRing::parse(&format!("v1:{k}")).unwrap();
        assert!(!format!("{ring:?}").contains(&k));
    }
}
