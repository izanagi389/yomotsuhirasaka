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
use hmac::Mac as _;

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

    /// 鍵のファイルにあるバージョンの一覧（昇順）。
    pub fn versions(&self) -> Vec<u32> {
        self.keys.keys().copied().collect()
    }

    fn cipher(&self, version: u32) -> Result<XChaCha20Poly1305> {
        let key = self
            .keys
            .get(&version)
            .ok_or_else(|| anyhow!("encryption key v{version} is not in the key file"))?;
        XChaCha20Poly1305::new_from_slice(key).map_err(|_| anyhow!("invalid key length"))
    }

    /// 用途ごとの鍵（現在の鍵を鍵にした HMAC-SHA256 で、用途の名前から導く）。鍵を入れ替えると変わる。
    /// 暗号化の鍵そのものを、別の用途（ハッシュの鍵など）に使わないため。
    pub fn derive(&self, purpose: &str) -> Result<[u8; KEY_BYTES]> {
        let key = self
            .keys
            .get(&self.active_version())
            .ok_or_else(|| anyhow!("no active key"))?;
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(key)
            .map_err(|_| anyhow!("invalid key length"))?;
        // 鍵の用途を分ける固定の文字列。製品の名前を変えても変えない（変えると導いた鍵がすべて変わる）
        mac.update(b"new-cms derived key\0");
        mac.update(purpose.as_bytes());
        Ok(mac.finalize().into_bytes().into())
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

/// 暗号文を作った鍵のバージョン（暗号文の先頭の4バイト）。
pub fn version_of(blob: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(blob.get(..4)?.try_into().ok()?))
}

fn random_key_line(version: u32) -> String {
    let mut key = [0u8; KEY_BYTES];
    rand::fill(&mut key);
    format!("v{version}:{}", STANDARD.encode(key))
}

/// 鍵の入れ替えの1段目: いちばん大きいバージョンの次の版で新しい鍵を加えた、鍵のファイルの中身を返す。
/// 既存の行（注釈を含む）はそのまま残す。
pub fn with_new_key(text: &str) -> Result<(String, u32)> {
    let version = KeyRing::parse(text)?.active_version() + 1;
    let mut out = text.to_owned();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&random_key_line(version));
    out.push('\n');
    Ok((out, version))
}

/// 鍵の入れ替えの3段目: 指定したバージョンの鍵の行を外した、鍵のファイルの中身を返す。
/// 新しく暗号化するときに使う鍵（いちばん大きいバージョン）は外せない。
pub fn without_version(text: &str, version: u32) -> Result<String> {
    let ring = KeyRing::parse(text)?;
    if !ring.keys.contains_key(&version) {
        bail!("encryption key v{version} is not in the key file");
    }
    if version == ring.active_version() {
        bail!("encryption key v{version} is the current key and cannot be removed");
    }
    let prefix = format!("v{version}:");
    let out: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().starts_with(&prefix))
        .collect();
    let out = format!("{}\n", out.join("\n"));
    KeyRing::parse(&out)?;
    Ok(out)
}

/// 鍵のファイルを置き換える。同じ場所に所有者だけが読める一時ファイルを作ってから名前を変えるので、
/// 途中で止まっても、古い中身か新しい中身のどちらかが残る。
pub fn replace_key_file(path: &Path, contents: &str) -> Result<()> {
    use std::io::Write as _;
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| anyhow!("invalid key file path"))?
        .to_string_lossy();
    let mut suffix = [0u8; 8];
    rand::fill(&mut suffix);
    let tmp = dir.join(format!(".{name}.{}.tmp", u64::from_be_bytes(suffix)));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut file = options
            .open(&tmp)
            .with_context(|| format!("cannot create {}", tmp.display()))?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("cannot replace {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
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
    fn key_file_editing() {
        let original = generate_key_file();
        let (added, v) = with_new_key(&original).unwrap();
        assert_eq!(v, 2);
        assert!(
            added.starts_with(&original),
            "existing lines and comments are kept"
        );
        let ring = KeyRing::parse(&added).unwrap();
        assert_eq!(ring.versions(), vec![1, 2]);
        // 古い版を外す。新しい版は外せない。ない版も外せない
        let removed = without_version(&added, 1).unwrap();
        assert_eq!(KeyRing::parse(&removed).unwrap().versions(), vec![2]);
        assert!(removed.contains("# CMS encryption keys"));
        assert!(without_version(&added, 2).is_err());
        assert!(without_version(&added, 7).is_err());
        assert!(without_version(&original, 1).is_err());
        // 新しい版で暗号化したものが、外した後も読める
        let blob = ring.encrypt(b"s", b"a").unwrap();
        assert_eq!(version_of(&blob), Some(2));
        assert_eq!(
            KeyRing::parse(&removed)
                .unwrap()
                .decrypt(&blob, b"a")
                .unwrap(),
            b"s"
        );
        assert_eq!(version_of(&[0, 0]), None);
    }

    #[test]
    fn key_file_is_replaced_privately() {
        let dir = std::env::temp_dir().join(format!("yomi-keys-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("encryption.key");
        replace_key_file(&path, "v1:x\n").unwrap();
        replace_key_file(&path, "v1:y\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v1:y\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "no temporary files are left"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn debug_does_not_print_keys() {
        let k = STANDARD.encode([9u8; KEY_BYTES]);
        let ring = KeyRing::parse(&format!("v1:{k}")).unwrap();
        assert!(!format!("{ring:?}").contains(&k));
    }
}
