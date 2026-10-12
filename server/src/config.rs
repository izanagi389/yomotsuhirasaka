//! サーバーの設定ファイル（TOML）。
//!
//! 秘密の値（DBのパスワード）は専用の項目に書き、接続先のURLには含めない。
//! 起動時の検査（`startup`）が、秘密の値を個別に確かめられるようにするため（Q168）。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// 本番の設定として起動するか。本番では、起動時の検査に1つでも当てはまれば起動しない。
    pub production: bool,
    /// 開発モード（任意のクエリを受け付ける。Q115）。本番の設定では有効にできない。
    #[serde(default)]
    pub dev_mode: bool,
    /// 公開するURL。本番では https であること（Q168）。
    pub public_url: String,
    /// 待ち受けるアドレス（リバースプロキシの後ろで動かす）。
    pub listen: String,
    /// ログの詳しさ（error, warn, info, debug, trace）。
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// 暗号化の鍵のファイル。DBやそのバックアップとは別に保管する（Q93, Q172）。
    pub encryption_key_file: Option<PathBuf>,
    /// 普段の処理用のDBの接続。
    pub database: DatabaseConfig,
    /// 移行とワークスペースのDBロールの作成に使う接続（Q173, Q179）。
    pub migration_database: DatabaseConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    /// 接続先のURL。パスワードは含めず、`password` に書く。
    pub url: String,
    /// 接続のパスワード（秘密の値）。
    pub password: String,
}

fn default_log_level() -> String {
    "info".to_owned()
}

/// 秘密の値の項目の名前（起動時の検査とテストのドライバーが使う）。
pub const SECRET_SETTING_NAMES: [&str; 2] = ["database.password", "migration_database.password"];

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read config file {}", path.display()))?;
        let mut config: Config = toml::from_str(&text)
            .with_context(|| format!("invalid config file {}", path.display()))?;
        // 相対パスは、作業ディレクトリではなく設定ファイルのあるディレクトリを基準にする
        if let (Some(key), Some(dir)) = (config.encryption_key_file.as_mut(), path.parent())
            && key.is_relative()
        {
            *key = dir.join(&*key);
        }
        Ok(config)
    }

    /// 秘密の値を、項目の名前とともに返す。
    pub fn secrets(&self) -> [(&'static str, &str); 2] {
        [
            (SECRET_SETTING_NAMES[0], self.database.password.as_str()),
            (
                SECRET_SETTING_NAMES[1],
                self.migration_database.password.as_str(),
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_key_path_is_resolved_from_config_dir() {
        let dir = std::env::temp_dir().join(format!("cms-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.toml");
        std::fs::write(
            &path,
            "production = true\npublic_url = \"https://x\"\nlisten = \"127.0.0.1:1\"\nencryption_key_file = \"k.key\"\n\
             [database]\nurl = \"postgres://a@h/d\"\npassword = \"p\"\n[migration_database]\nurl = \"postgres://b@h/d\"\npassword = \"q\"\n",
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.encryption_key_file.unwrap(), dir.join("k.key"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let text = "production = true\npublic_url = \"https://x\"\nlisten = \"a\"\ntypo = 1\n[database]\nurl = \"u\"\npassword = \"p\"\n[migration_database]\nurl = \"u\"\npassword = \"p\"\n";
        assert!(toml::from_str::<Config>(text).is_err());
    }
}
