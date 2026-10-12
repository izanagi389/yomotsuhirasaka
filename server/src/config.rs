//! サーバーの設定ファイル（TOML）。
//!
//! 秘密の値（DBのパスワード）は専用の項目に書き、接続先のURLには含めない。
//! 起動時の検査（`startup`）が、秘密の値を個別に確かめられるようにするため（Q168）。

use std::net::IpAddr;
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
    /// `X-Forwarded-For` を信頼するプロキシ（リバースプロキシ）のIPアドレス（Q106）。
    /// ここにないところから届いた `X-Forwarded-For` は無視し、接続元のアドレスを使う。
    #[serde(default)]
    pub trusted_proxies: Vec<IpAddr>,
    /// セッションと再認証の時間（Q91, Q108）。
    #[serde(default)]
    pub session: SessionConfig,
    /// 回数制限（Q105）。
    #[serde(default)]
    pub limits: LimitsConfig,
    /// 認証の方針。
    #[serde(default)]
    pub auth: AuthConfig,
    /// メールの送信（SMTP）。なければメールを送らない（通知は送らず、リセットや招待のリンクは画面に出す）。
    #[serde(default)]
    pub smtp: Option<SmtpConfig>,
}

/// 認証の方針。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct AuthConfig {
    /// 全員に多要素認証を必須にする（Q197。既定で有効）。
    /// 有効なとき、多要素認証のない利用者は、その設定に必要な操作しかできない。
    pub mfa_required_for_everyone: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            mfa_required_for_everyone: true,
        }
    }
}

/// SMTPの接続の守り方。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SmtpSecurity {
    /// 平文で接続し、必ずSTARTTLSに切り替える（切り替えられなければ送らない）。既定のポートは587。
    #[default]
    Starttls,
    /// 最初からTLSで接続する。既定のポートは465。
    Tls,
    /// 暗号化しない。同じホストの中継など、ループバックの宛先にだけ使う（本番の設定では、それ以外は起動を止める）。
    None,
}

/// メールの送信の設定。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SmtpConfig {
    pub host: String,
    pub port: Option<u16>,
    #[serde(default)]
    pub security: SmtpSecurity,
    pub username: Option<String>,
    /// 秘密の値。
    pub password: Option<String>,
    /// 差出人（`CMS <cms@example.com>` か `cms@example.com`）。
    pub from: String,
}

/// セッションと再認証の時間（秒）。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct SessionConfig {
    /// 絶対的な期限（ログインからの最大時間）。
    pub absolute_lifetime_seconds: u32,
    /// 無操作の期限。
    pub idle_timeout_seconds: u32,
    /// 再認証が有効な時間。重い操作は、最後の本人確認からこの時間内にだけ行える。
    pub reauth_window_seconds: u32,
    /// リセット・招待のリンクの有効期限（Q104）。
    pub one_time_token_lifetime_seconds: u32,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            absolute_lifetime_seconds: 12 * 60 * 60,
            idle_timeout_seconds: 2 * 60 * 60,
            reauth_window_seconds: 10 * 60,
            one_time_token_lifetime_seconds: 30 * 60,
        }
    }
}

/// 回数制限。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct LimitsConfig {
    /// 接続元のIPアドレス（IPv6は /64）ごとの、1分あたりのログインの試行の上限（成功・失敗を問わない）。
    pub login_attempts_per_ip_per_minute: u32,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            login_attempts_per_ip_per_minute: 30,
        }
    }
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
