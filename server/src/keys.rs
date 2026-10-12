//! 暗号化の鍵のファイル。
//!
//! 1行に1つの鍵を `v<バージョン>:<32バイトのBase64>` の形で書く。`#` で始まる行は注釈。
//! いちばん大きいバージョンを、新しく暗号化するときに使う（鍵の入れ替え、Q93）。
//! 鍵がない・読めない・形が正しくない場合、サーバーは起動しない（INV-KEY-02）。

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

pub const KEY_BYTES: usize = 32;

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
    fn debug_does_not_print_keys() {
        let k = STANDARD.encode([9u8; KEY_BYTES]);
        let ring = KeyRing::parse(&format!("v1:{k}")).unwrap();
        assert!(!format!("{ring:?}").contains(&k));
    }
}
