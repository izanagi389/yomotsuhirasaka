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
    /// インスタンスの既定の言語（Q198）。言語を設定していない利用者や、まだ利用者でない宛先へのメールに使う。
    #[serde(default)]
    pub default_locale: crate::locale::Locale,
    /// 認証の方針。
    #[serde(default)]
    pub auth: AuthConfig,
    /// メールの送信（SMTP）。なければメールを送らない（通知は送らず、リセットや招待のリンクは画面に出す）。
    #[serde(default)]
    pub smtp: Option<SmtpConfig>,
    /// ログインの履歴（Q176）。
    #[serde(default)]
    pub login_history: LoginHistoryConfig,
    /// メディア（13-security.md「メディア」）。
    pub media: MediaConfig,
    /// 配信API（13-security.md「GraphQL」）。
    #[serde(default)]
    pub delivery: DeliveryConfig,
}

/// 配信APIの上限と、キャッシュの扱い。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct DeliveryConfig {
    /// クエリの深さの上限（既定は15。深さ25のクエリは必ず拒否する大きさにする。Q186）。
    pub max_depth: usize,
    /// クエリの複雑さ（選ぶフィールドの数）の上限（既定は500。別名1000個のクエリは必ず拒否する大きさにする。Q186）。
    pub max_complexity: usize,
    /// APIキーごとの1分あたりの回数の上限。用途がビルド用のキー（既定は600）。
    pub build_key_requests_per_minute: u32,
    /// 同じく、用途がサーバー配信用（SSRなど）のキー（既定は6000。Q145, Q153）。
    pub server_key_requests_per_minute: u32,
    /// ワークスペースごとの1分あたりの回数の上限（キーをまたいだ合計。既定は30000。Q120）。
    pub workspace_requests_per_minute: u32,
    /// 応答の大きさの上限（バイト。既定は5MB）。超える応答は返さない。
    pub max_response_bytes: usize,
    /// 共有キャッシュ（CDNなど）に載せることを許すか（既定は許さない。Q125）。許しても、公開版の読み取りだけを範囲とするキーの
    /// 応答に限る。共有キャッシュに載った応答は、APIキーの照合なしに、URLを知っている人なら誰でも取得できる。
    pub shared_cache: bool,
    /// 共有キャッシュに載せる時間（秒。既定は60）。
    pub shared_cache_seconds: u32,
    /// テンプレートのデプロイで、登録済みのクエリを検証する範囲。この日数より長く使われていないクエリは検証しない（既定は90日。Q151）。
    pub query_check_unused_days: u32,
}

impl Default for DeliveryConfig {
    fn default() -> Self {
        Self {
            max_depth: 15,
            max_complexity: 500,
            build_key_requests_per_minute: 600,
            server_key_requests_per_minute: 6000,
            workspace_requests_per_minute: 30_000,
            max_response_bytes: 5 * 1024 * 1024,
            shared_cache: false,
            shared_cache_seconds: 60,
            query_check_unused_days: 90,
        }
    }
}

/// メディアの保存先と上限。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MediaConfig {
    /// 保存先のディレクトリ（ローカルのファイルシステムのみ）。ワークスペースごとのディレクトリをこの下に作る。
    pub root: PathBuf,
    /// 配信に使うURLの先頭（例: `https://media.example.com`）。管理画面とは別のドメインを推奨する（Q96）。
    /// 省略時は `public_url` の `/media`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_base_url: Option<String>,
    /// 1枚の画像の画素数の上限（既定は5千万）。
    #[serde(default = "default_max_pixels")]
    pub max_pixels: u64,
    /// 1つのファイルの大きさの上限（バイト。既定は20MB）。
    #[serde(default = "default_max_file_bytes")]
    pub max_file_bytes: u64,
    /// ワークスペースの容量の既定の上限（バイト。既定は5GB）。サーバー管理者がワークスペースごとに変えられる。
    #[serde(default = "default_workspace_quota_bytes")]
    pub default_workspace_quota_bytes: u64,
    /// 画像の処理（デコードとエンコードし直し）にかける時間の上限（秒。既定は20秒）。
    #[serde(default = "default_processing_timeout_seconds")]
    pub processing_timeout_seconds: u32,
}

fn default_max_pixels() -> u64 {
    50_000_000
}
fn default_max_file_bytes() -> u64 {
    20 * 1024 * 1024
}
fn default_workspace_quota_bytes() -> u64 {
    5 * 1024 * 1024 * 1024
}
fn default_processing_timeout_seconds() -> u32 {
    20
}

impl MediaConfig {
    /// 保存先だけを指定し、上限は既定値にした設定。
    pub fn at(root: PathBuf) -> Self {
        Self {
            root,
            public_base_url: None,
            max_pixels: default_max_pixels(),
            max_file_bytes: default_max_file_bytes(),
            default_workspace_quota_bytes: default_workspace_quota_bytes(),
            processing_timeout_seconds: default_processing_timeout_seconds(),
        }
    }
}

/// ログインの履歴の保存期間と、削除の間隔（秒）。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct LoginHistoryConfig {
    /// 保存期間（既定は90日）。過ぎたものは自動で削除する。
    pub retention_seconds: u32,
    /// 期限を過ぎた履歴を探して削除する間隔（既定は1時間）。
    pub sweep_interval_seconds: u32,
    /// 国を調べるデータ（MaxMind DB 形式。GeoLite2-Country など）の置き場所。
    /// 指定したときだけ、履歴に国を残し、国の変化を本人に知らせる（Q175）。データは同梱しない。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub geoip_database: Option<PathBuf>,
}

impl Default for LoginHistoryConfig {
    fn default() -> Self {
        Self {
            retention_seconds: 90 * 24 * 60 * 60,
            sweep_interval_seconds: 60 * 60,
            geoip_database: None,
        }
    }
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
        if let Some(dir) = path.parent()
            && config.media.root.is_relative()
        {
            config.media.root = dir.join(&config.media.root);
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
             [database]\nurl = \"postgres://a@h/d\"\npassword = \"p\"\n[migration_database]\nurl = \"postgres://b@h/d\"\npassword = \"q\"\n[media]\nroot = \"media\"\n",
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.encryption_key_file.unwrap(), dir.join("k.key"));
        assert_eq!(config.media.root, dir.join("media"));
        assert_eq!(config.media.max_pixels, 50_000_000);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let text = "production = true\npublic_url = \"https://x\"\nlisten = \"a\"\ntypo = 1\n[database]\nurl = \"u\"\npassword = \"p\"\n[migration_database]\nurl = \"u\"\npassword = \"p\"\n";
        assert!(toml::from_str::<Config>(text).is_err());
    }
}
