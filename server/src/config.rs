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
    /// 回答API（13-security.md「チャットボットへの悪用」。M4）。
    #[serde(default)]
    pub answer: AnswerConfig,
    /// 外部へのHTTPの送信（13-security.md「外部への送信」）。
    #[serde(default)]
    pub egress: EgressConfig,
    /// LLMの接続先のインスタンスの既定値（10-llm-and-budget.md）。なければ埋め込みを作らない。
    /// 管理画面や管理APIからは変えられない（ワークスペースごとの上書きはM4）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm: Option<LlmConfig>,
    /// 検索と関連記事（08-search-and-related.md）。
    #[serde(default)]
    pub search: SearchConfig,
    /// 管理画面（Nuxt の SPA）の配信。
    #[serde(default)]
    pub admin: AdminConfig,
    /// 組み込みJS（04-hooks.md、Q89）。
    #[serde(default)]
    pub scripts: ScriptsConfig,
    /// 監査ログの外部への書き出し（11-permissions-and-audit.md「外部への書き出し」、Q81）。インスタンス全体の記録が出る。
    #[serde(default)]
    pub audit_export: AuditExportConfig,
    /// 監査ログの保存期間（11-permissions-and-audit.md「保護と保存期間」）。
    #[serde(default)]
    pub audit: AuditConfig,
    /// 管理者への通知の急増のしきい値（13-security.md「管理者への通知」、計画の15番）。
    #[serde(default)]
    pub alerts: AlertsConfig,
}

/// 急増のしきい値。window_seconds の間の件数が、しきい値以上になったら知らせる。同じ種類は cooldown_seconds に1回まで。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct AlertsConfig {
    /// ログインの失敗（パスワードと2段目の両方）の件数。
    pub login_failures: i64,
    /// 回数制限への到達（429）の件数。
    pub rate_limited: i64,
    pub window_seconds: i64,
    pub cooldown_seconds: i64,
}

impl Default for AlertsConfig {
    fn default() -> Self {
        Self {
            login_failures: 30,
            rate_limited: 100,
            window_seconds: 300,
            cooldown_seconds: 1800,
        }
    }
}

/// 検索と関連記事。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct SearchConfig {
    /// 関連記事の毎日の一括計算を始める時刻（UTCの時、0〜23。既定は18 = 日本時間の3時）。
    pub related_batch_hour_utc: u8,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            related_batch_hour_utc: 18,
        }
    }
}

/// 監査ログの保存期間。過ぎたものは自動で削除する（専用のロールの関数で。書き出し先に送り終えていないものは残す）。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct AuditConfig {
    /// 保存する日数（既定は365、30以上）。インスタンス全体で1つ。
    pub retention_days: i32,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            retention_days: 365,
        }
    }
}

/// 監査ログの外部への書き出し（インスタンス全体。運用者が決める）。1行1件の JSON。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct AuditExportConfig {
    /// 標準出力に書く（サーバーのログは標準エラー出力なので混ざらない）。
    pub stdout: bool,
    /// 追記するファイル。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// 送るWebhook（署名付き）。
    pub webhooks: Vec<AuditWebhookConfig>,
}

/// 監査ログを送るWebhook。署名の秘密は別のファイルに置く（設定ファイルに直接書かない）。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuditWebhookConfig {
    pub url: String,
    pub secret_file: PathBuf,
}

/// 組み込みJSの1回の実行あたりの上限（Q89）。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct ScriptsConfig {
    /// メモリの上限（バイト。既定は128MB）。
    pub memory_limit_bytes: usize,
    /// 同期の実行（URL生成関数、保存前・公開前のフック）の時間の上限（ミリ秒。既定は5000）。ホスト関数で待つ時間も含む。
    pub time_limit_ms: u64,
    /// 非同期のフック（保存後・公開後・非公開の後）の時間の上限（ミリ秒。既定は30000）。外部への送信を待つ時間も含む。
    pub async_time_limit_ms: u64,
}

impl Default for ScriptsConfig {
    fn default() -> Self {
        Self {
            memory_limit_bytes: 128 * 1024 * 1024,
            time_limit_ms: 5000,
            async_time_limit_ms: 30_000,
        }
    }
}

impl ScriptsConfig {
    pub fn limits(&self) -> crate::script::Limits {
        crate::script::Limits {
            memory_bytes: self.memory_limit_bytes,
            time: std::time::Duration::from_millis(self.time_limit_ms),
            async_time: std::time::Duration::from_millis(self.async_time_limit_ms),
        }
    }
}

/// 管理画面の配信。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct AdminConfig {
    /// `nuxt generate` の出力（`admin/.output/public`）。なければ管理画面を配信しない（管理APIは使える）。
    pub dist_dir: Option<PathBuf>,
}

/// 外部へのHTTPの送信の共通の仕組みの設定（Q95, Q112, Q128, Q139）。
/// プロキシの環境変数（`HTTP_PROXY` など）は読まない。出口のプロキシは、ここで明示したときだけ使う。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct EgressConfig {
    /// サーバー管理者が設定した宛先（LLM、OIDC、インスタンスの監査ログの書き出し先）で、例外として送信を許す内部の宛先。
    /// ホスト名かIPアドレスで、`:ポート` を付けるとそのポートだけになる。例外の宛先には、HTTP（HTTPSでない）での送信も許す（Q139）。
    pub internal_destination_exceptions: Vec<String>,
    /// ワークスペースの管理者が決める宛先（Webhook、フックの送信、ワークスペースの監査ログの書き出し先）で、
    /// 例外として送信を許す内部の宛先（書き方は同じ）。自前のビルドサーバーなど。どのワークスペースからも送れるようになる。
    pub workspace_internal_destination_exceptions: Vec<String>,
    /// 標準の一覧（Mozilla）に加えて信頼する、CAの証明書（PEM）のファイル。社内のCAなど。
    pub extra_ca_files: Vec<PathBuf>,
    /// 接続（名前解決したアドレスへのTCPの接続とTLSの確立）の上限の時間（秒）。
    pub connect_timeout_seconds: u32,
    /// 1回の送信全体の上限の時間（秒）。
    pub request_timeout_seconds: u32,
    /// 出口のプロキシ（Q128）。
    pub proxy: Option<EgressProxyConfig>,
}

impl Default for EgressConfig {
    fn default() -> Self {
        Self {
            internal_destination_exceptions: Vec::new(),
            workspace_internal_destination_exceptions: Vec::new(),
            extra_ca_files: Vec::new(),
            connect_timeout_seconds: 10,
            request_timeout_seconds: 30,
            proxy: None,
        }
    }
}

/// 出口のプロキシ（Q128）。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EgressProxyConfig {
    /// プロキシのURL（`http://host:port`）。認証情報は書かない（暗号化して保存する方式は未対応）。
    pub url: String,
    /// 中継の依頼（CONNECT）の宛先の指定の仕方。既定は、CMSが検査したIPアドレス。
    #[serde(default)]
    pub connect_by: ProxyConnectBy,
}

/// 中継の依頼の宛先。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyConnectBy {
    /// CMSが名前解決して検査したIPアドレスで依頼する（検査したアドレスへの固定を保つ）。
    #[default]
    Ip,
    /// ホスト名で依頼する（ホスト名でしか中継を許さないプロキシ向け）。検査の後に名前解決の結果が変わる攻撃を防げるかは、プロキシ次第になる。
    Hostname,
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
    /// 1回のクエリの実行時間の上限（秒。既定は15）。
    pub max_query_seconds: u64,
    /// 1回のクエリで解決する記事の数の上限（既定は10000）。一覧と参照の組み合わせで件数が掛け算で増えるのを止める。
    pub max_resolved_entries: usize,
    /// ワークスペースごとの、同時に実行するクエリの数の上限（既定は6）。
    pub max_concurrent_queries_per_workspace: u32,
    /// インスタンス全体の、同時に実行するクエリの数の上限（既定は12。DBの接続は全体で16本なので、管理画面とジョブの分を残す）。
    pub max_concurrent_queries: u32,
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
            max_query_seconds: 15,
            max_resolved_entries: 10_000,
            max_concurrent_queries_per_workspace: 6,
            max_concurrent_queries: 12,
        }
    }
}

/// 回答APIの回数制限（13-security.md「チャットボットへの悪用」、Q116）。配信APIの回数制限とは別に数える。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct AnswerConfig {
    /// APIキーごとの、1分あたりの回答の回数の上限（既定は60）。
    pub key_requests_per_minute: u32,
    /// 訪問者の識別子（中継サーバーが渡すもの）ごとの、1分あたりの回答の回数の上限（既定は10）。
    pub visitor_requests_per_minute: u32,
    /// 同じく、1日あたり（既定は100）。1人の訪問者による予算の食いつぶしを止める。
    pub visitor_requests_per_day: u32,
}

impl Default for AnswerConfig {
    fn default() -> Self {
        Self {
            key_requests_per_minute: 60,
            visitor_requests_per_minute: 10,
            visitor_requests_per_day: 100,
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
    /// ワークスペースごとの容量の上限（バイト。既定は5GB）。値はインスタンスで1つ。
    #[serde(default = "default_workspace_quota_bytes")]
    pub default_workspace_quota_bytes: u64,
    /// 画像の処理（デコードとエンコードし直し）にかける時間の上限（秒。既定は20秒）。
    #[serde(default = "default_processing_timeout_seconds")]
    pub processing_timeout_seconds: u32,
    /// 同時に配信するファイルの数の上限（既定は512）。超えた分は 503 を返す。
    #[serde(default = "default_max_concurrent_serves")]
    pub max_concurrent_serves: u32,
    /// 接続元のIPアドレス（IPv6は /64）ごとの、1分あたりのメディアのリクエストの上限（既定は1200）。超えたら 429。
    /// 0 なら数えない。CDN を前に置くなら、CDN を `trusted_proxies` に入れる（入れないと CDN の分がまとめて数えられる）。
    #[serde(default = "default_requests_per_ip_per_minute")]
    pub requests_per_ip_per_minute: u32,
}

fn default_requests_per_ip_per_minute() -> u32 {
    1200
}

fn default_max_concurrent_serves() -> u32 {
    512
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
            max_concurrent_serves: default_max_concurrent_serves(),
            requests_per_ip_per_minute: default_requests_per_ip_per_minute(),
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
    /// 漏えいしたパスワードの照会（Q170）。なければ照会しない（既定）。
    pub breached_passwords: Option<BreachedPasswordsConfig>,
    /// 外部のOIDC/SSO（Q103, Q126）。なければSSOは使えない（既定）。
    pub oidc: Option<OidcConfig>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            mfa_required_for_everyone: true,
            breached_passwords: None,
            oidc: None,
        }
    }
}

/// 外部のOIDC/SSOの接続先（13-security.md「OIDC/SSO」）。IdPの設定の取得も、外部への送信の共通の仕組みを通る
/// （社内のIdPなら、`[egress]` の内部宛先の例外に加える）。
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OidcConfig {
    /// IdPの発行者（`iss`）。`<issuer>/.well-known/openid-configuration` から設定を取る。
    pub issuer: String,
    pub client_id: String,
    /// クライアントの秘密（秘密の値）。
    pub client_secret: String,
    /// ログインの画面のボタンに出す名前。
    #[serde(default = "default_oidc_display_name")]
    pub display_name: String,
    /// JIT: IDトークンの `email_verified` が真で、メールアドレスのドメインがこれに一致すれば、
    /// 初回のSSOログインでアカウントを作る。空なら招待制（既定）。
    #[serde(default)]
    pub jit_domains: Vec<String>,
}

/// クライアントの秘密は出さない（設定をログなどに出したときに漏れないように）。
impl std::fmt::Debug for OidcConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcConfig")
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("display_name", &self.display_name)
            .field("jit_domains", &self.jit_domains)
            .finish()
    }
}

fn default_oidc_display_name() -> String {
    "SSO".to_owned()
}

/// LLMの接続先（OpenAI互換のAPI）。埋め込みと、回答の生成（チャット。M4）。送信は外部への送信の共通の仕組みを通す。
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LlmConfig {
    /// OpenAI互換のAPIの接続先（例: `https://api.openai.com/v1`）。本番では https。
    /// 外部への送信の例外として許した内部の宛先（擬似LLMやローカルのLLM）だけ http も許す。
    pub base_url: String,
    /// APIキー（秘密の値）。空にはできない。
    pub api_key: String,
    /// 埋め込みのモデル名。
    pub embedding_model: String,
    /// 埋め込みの次元数（1〜2000。pgvector の HNSW の索引の上限）。応答のベクトルの長さと照合する。
    pub embedding_dimensions: u32,
    /// 埋め込みの単価（100万トークンあたり。任意）。あれば見積もりで費用も出す（Q138）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_price_per_million_tokens: Option<f64>,
    /// ワークスペースの埋め込みの作成の予算の天井の既定値（トークン数、UTCの暦の月）。
    /// サーバー管理者がワークスペースごとに決めていなければ、これが効く。なければ 0（埋め込みを作らない）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_workspace_embedding_tokens_per_month: Option<u64>,
    /// 同じく、UTCの暦の日。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_workspace_embedding_tokens_per_day: Option<u64>,
    /// 回答の生成に使うチャットのモデル名（M4、09-qa-chatbot.md）。なければ回答APIを使えない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_model: Option<String>,
    /// チャットの単価（100万トークンあたり、入力・出力。任意）。あれば費用も出す（Q138）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_input_price_per_million_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_output_price_per_million_tokens: Option<f64>,
    /// ワークスペースの訪問者向けの回答の予算の天井の既定値（トークン数、入力・出力別、UTCの暦の月・日）。
    /// サーバー管理者がワークスペースごとに決めていなければ、これが効く。なければ 0（回答しない）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_workspace_answer_input_tokens_per_month: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_workspace_answer_input_tokens_per_day: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_workspace_answer_output_tokens_per_month: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_workspace_answer_output_tokens_per_day: Option<u64>,
    /// ワークスペースの編集者向けの支援の予算の天井の既定値（トークン数、入力・出力別、UTCの暦の月・日。M4、Q70）。
    /// サーバー管理者がワークスペースごとに決めていなければ、これが効く。なければ 0（支援で LLM を呼ばない）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_workspace_assist_input_tokens_per_month: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_workspace_assist_input_tokens_per_day: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_workspace_assist_output_tokens_per_month: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_workspace_assist_output_tokens_per_day: Option<u64>,
    /// 切り替え先の埋め込みのモデル（08-search-and-related.md「モデルの変更」、Q30）。書いただけでは何もしない。
    /// サーバー管理者が見積もりを見て開始すると、ワークスペースごとに新しい埋め込みを作り、そろった時点で切り替える。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_embedding: Option<NextEmbeddingConfig>,
    /// ワークスペースが LLM の接続設定の上書きで選べる接続先（`[[llm.allowed_endpoints]]`。M4、10-llm-and-budget.md
    /// 「接続設定の単位」、Q26）。ワークスペースは名前で選び、URL は書けない（SSRF対策）。サーバー管理者が設定ファイルで決める。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_endpoints: Vec<AllowedEndpoint>,
    /// ワークスペースが上書きで選べる埋め込みのモデル（`[[llm.allowed_embedding_models]]`。M4）。
    /// 起動のときに、モデルごとの検索の索引を作っておく（強い権限の接続を、ふだんの処理から使わないため）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_embedding_models: Vec<AllowedEmbeddingModel>,
}

/// ワークスペースが選べる埋め込みのモデル（`[[llm.allowed_embedding_models]]`）。
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AllowedEmbeddingModel {
    pub model: String,
    /// 次元数（1〜2000）。応答のベクトルの長さと照合する。
    pub dimensions: u32,
    /// 単価（100万トークンあたり。任意）。あれば見積もりで費用も出す。
    #[serde(default)]
    pub embedding_price_per_million_tokens: Option<f64>,
}

/// ワークスペースが選べる LLM の接続先（`[[llm.allowed_endpoints]]`）。
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AllowedEndpoint {
    /// ワークスペースが選ぶときの名前。
    pub name: String,
    /// OpenAI互換のAPIの接続先。本番では https（内部の宛先は外部への送信の例外に入れたものだけ http）。
    pub base_url: String,
}

/// 切り替え先の埋め込みのモデル（`[llm.next_embedding]`）。
/// 接続先とキーは、省略すれば今のもの（`[llm]`）を使う。接続先を書くときはキーも書く（今のキーを別の宛先に送らないため）。
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NextEmbeddingConfig {
    pub model: String,
    pub dimensions: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// APIキー（秘密の値）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_price_per_million_tokens: Option<f64>,
}

impl std::fmt::Debug for NextEmbeddingConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NextEmbeddingConfig")
            .field("model", &self.model)
            .field("dimensions", &self.dimensions)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field(
                "embedding_price_per_million_tokens",
                &self.embedding_price_per_million_tokens,
            )
            .finish()
    }
}

impl LlmConfig {
    /// 今のモデルの接続先（切り替え先を外したもの）。
    pub fn current(&self) -> LlmConfig {
        LlmConfig {
            next_embedding: None,
            ..self.clone()
        }
    }

    /// 切り替え先のモデルの接続先（なければ None）。接続先・キーを省略していれば、今のものを使う。
    /// 接続先だけを書いてキーがない設定は、起動のときの検査で止める（ここでは使わない）。
    pub fn next(&self) -> Option<LlmConfig> {
        let n = self.next_embedding.as_ref()?;
        if n.base_url.is_some() && n.api_key.is_none() {
            return None;
        }
        Some(LlmConfig {
            base_url: n.base_url.clone().unwrap_or_else(|| self.base_url.clone()),
            api_key: n.api_key.clone().unwrap_or_else(|| self.api_key.clone()),
            embedding_model: n.model.clone(),
            embedding_dimensions: n.dimensions,
            embedding_price_per_million_tokens: n.embedding_price_per_million_tokens,
            next_embedding: None,
            ..self.clone()
        })
    }

    /// そのモデルと次元数の接続先（設定の今のモデルか切り替え先のモデル。どちらでもなければ None）。
    pub fn for_model(&self, model: &str, dimensions: u32) -> Option<LlmConfig> {
        [Some(self.current()), self.next()]
            .into_iter()
            .flatten()
            .find(|c| c.embedding_model == model && c.embedding_dimensions == dimensions)
    }
}

/// APIキーは出さない（設定をログなどに出したときに漏れないように）。
impl std::fmt::Debug for LlmConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmConfig")
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .field("embedding_model", &self.embedding_model)
            .field("embedding_dimensions", &self.embedding_dimensions)
            .field(
                "embedding_price_per_million_tokens",
                &self.embedding_price_per_million_tokens,
            )
            .field(
                "default_workspace_embedding_tokens_per_month",
                &self.default_workspace_embedding_tokens_per_month,
            )
            .field(
                "default_workspace_embedding_tokens_per_day",
                &self.default_workspace_embedding_tokens_per_day,
            )
            .field("chat_model", &self.chat_model)
            .field(
                "chat_input_price_per_million_tokens",
                &self.chat_input_price_per_million_tokens,
            )
            .field(
                "chat_output_price_per_million_tokens",
                &self.chat_output_price_per_million_tokens,
            )
            .field(
                "default_workspace_answer_input_tokens_per_month",
                &self.default_workspace_answer_input_tokens_per_month,
            )
            .field(
                "default_workspace_answer_input_tokens_per_day",
                &self.default_workspace_answer_input_tokens_per_day,
            )
            .field(
                "default_workspace_answer_output_tokens_per_month",
                &self.default_workspace_answer_output_tokens_per_month,
            )
            .field(
                "default_workspace_answer_output_tokens_per_day",
                &self.default_workspace_answer_output_tokens_per_day,
            )
            .field(
                "default_workspace_assist_input_tokens_per_month",
                &self.default_workspace_assist_input_tokens_per_month,
            )
            .field(
                "default_workspace_assist_input_tokens_per_day",
                &self.default_workspace_assist_input_tokens_per_day,
            )
            .field(
                "default_workspace_assist_output_tokens_per_month",
                &self.default_workspace_assist_output_tokens_per_month,
            )
            .field(
                "default_workspace_assist_output_tokens_per_day",
                &self.default_workspace_assist_output_tokens_per_day,
            )
            .field("next_embedding", &self.next_embedding)
            .finish()
    }
}

/// 埋め込みの次元数の上限（pgvector の `vector` 型の HNSW の索引の上限）。
pub const MAX_EMBEDDING_DIMENSIONS: u32 = 2000;

/// 漏えいしたパスワードの照会先（Have I Been Pwned の range API と同じ形）。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BreachedPasswordsConfig {
    /// 照会先（例: `https://api.pwnedpasswords.com`）。`<base_url>/range/<SHA-1の先頭5文字>` を引く。
    pub base_url: String,
    /// 照会を待つ上限（秒）。超えたら照会せずに登録を通す。
    #[serde(default = "default_breached_timeout")]
    pub timeout_seconds: u32,
}

fn default_breached_timeout() -> u32 {
    3
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
    /// 差出人（`CMS <yomi@example.com>` か `yomi@example.com`）。
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
    /// 利用者ごとの、1分あたりの管理APIのリクエストの上限（Cookieのセッションと個人トークンを合わせて数える）。
    pub admin_requests_per_user_per_minute: u32,
    /// ワークスペースごとの、1分あたりの管理APIのリクエストの上限（所属する人のリクエストだけを数える）。
    pub admin_requests_per_workspace_per_minute: u32,
    /// ワークスペースごとの記事の数の上限（翻訳は別の記事として数える。削除して物理削除を待つ記事も含む）。
    pub max_entries_per_workspace: u32,
    /// ワークスペースごとの登録クエリの数の上限。
    pub max_queries_per_workspace: u32,
    /// ワークスペースごとの、1分あたりのクエリの新規登録の上限（登録済みの文を登録し直すときは数えない）。
    pub query_registrations_per_workspace_per_minute: u32,
    /// 接続元のIPアドレス（IPv6は /64）ごとの、1分あたりの無効な配信APIのキーの上限。
    pub invalid_api_keys_per_ip_per_minute: u32,
    /// リクエストの処理時間の上限（秒。本文の読み込みを含む。ストリームの本文は含まない。デプロイと取り込みは除く）。
    pub request_timeout_seconds: u64,
    /// 同時に処理するリクエストの数の上限。超えたら待たせずに断る。
    pub max_concurrent_requests: u32,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            login_attempts_per_ip_per_minute: 30,
            admin_requests_per_user_per_minute: 600,
            admin_requests_per_workspace_per_minute: 3000,
            max_entries_per_workspace: 100_000,
            max_queries_per_workspace: 1000,
            query_registrations_per_workspace_per_minute: 60,
            invalid_api_keys_per_ip_per_minute: 600,
            request_timeout_seconds: 120,
            max_concurrent_requests: 2048,
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
        if let (Some(dir), Some(ui)) = (path.parent(), config.admin.dist_dir.as_mut())
            && ui.is_relative()
        {
            *ui = dir.join(&*ui);
        }
        if let Some(dir) = path.parent() {
            for ca in &mut config.egress.extra_ca_files {
                if ca.is_relative() {
                    *ca = dir.join(&*ca);
                }
            }
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
        let dir = std::env::temp_dir().join(format!("yomi-config-test-{}", std::process::id()));
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
