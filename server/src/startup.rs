//! 起動時の検査（Q168, Q173, Q179, Q93）。
//!
//! 本番の設定では、次のどれかに当てはまれば理由を表示して起動を止める（INV-CFG-01）:
//! 秘密の値が空・短すぎる・既知の初期値、DBの接続ユーザー（普段・移行用）がスーパーユーザー、
//! PostgreSQLが16より古い、開発モードが有効、HTTPS前提の設定がない。
//! 開発の設定では、同じ検査を警告にとどめる。
//! 暗号化の鍵がない（INV-KEY-02）、PostgreSQLが16より古い（Q179）は、設定によらず起動を止める。

use std::time::Duration;

use sqlx::postgres::{PgConnectOptions, PgConnection};
use sqlx::{ConnectOptions as _, Connection as _};

use crate::config::{Config, DatabaseConfig};
use crate::keys::KeyRing;

pub const MIN_POSTGRES_VERSION_NUM: i32 = 160_000;
pub const MIN_SECRET_LENGTH: usize = 16;

/// よく知られた初期値・弱い値（小文字で比べる）。
const KNOWN_WEAK_SECRETS: &[&str] = &[
    "password",
    "passw0rd",
    "changeme",
    "change-me",
    "change_me",
    "secret",
    "admin",
    "administrator",
    "postgres",
    "root",
    "cms",
    "test",
    "example",
    "default",
    "letmein",
    "qwerty",
    "123456",
    "12345678",
    "1234567890",
];

/// 検査で見つかった問題。`fatal` は設定によらず起動を止める。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub message: String,
    pub fatal: bool,
}

impl Problem {
    fn config(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            fatal: false,
        }
    }
    fn fatal(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            fatal: true,
        }
    }
}

/// 検査の結果。
#[derive(Debug, Default)]
pub struct Report {
    pub problems: Vec<Problem>,
}

impl Report {
    /// 起動を止めるべき問題。本番の設定では、すべての問題が起動を止める。
    pub fn blocking(&self, production: bool) -> Vec<&Problem> {
        self.problems
            .iter()
            .filter(|p| p.fatal || production)
            .collect()
    }
    /// 警告にとどめる問題（開発の設定のときだけ）。
    pub fn warnings(&self, production: bool) -> Vec<&Problem> {
        self.problems
            .iter()
            .filter(|p| !p.fatal && !production)
            .collect()
    }
}

/// 秘密の値の検査。値そのものは問題の文に含めない。
pub fn check_secret(name: &str, value: &str) -> Option<Problem> {
    if value.is_empty() {
        return Some(Problem::config(format!("secret `{name}` is empty")));
    }
    if value.chars().count() < MIN_SECRET_LENGTH {
        return Some(Problem::config(format!(
            "secret `{name}` is shorter than {MIN_SECRET_LENGTH} characters"
        )));
    }
    let lower = value.to_lowercase();
    if KNOWN_WEAK_SECRETS.contains(&lower.as_str()) {
        return Some(Problem::config(format!(
            "secret `{name}` is a well-known default value"
        )));
    }
    let first = lower.chars().next();
    if lower.chars().all(|c| Some(c) == first) {
        return Some(Problem::config(format!(
            "secret `{name}` repeats a single character"
        )));
    }
    None
}

/// 設定ファイルだけで判断できる検査。
pub fn check_config(config: &Config) -> Report {
    let mut report = Report::default();
    for (name, value) in config.secrets() {
        report.problems.extend(check_secret(name, value));
    }
    for (name, db) in [
        ("database", &config.database),
        ("migration_database", &config.migration_database),
    ] {
        match db.url.parse::<PgConnectOptions>() {
            Err(_) => report.problems.push(Problem::fatal(format!(
                "`{name}.url` is not a valid PostgreSQL URL"
            ))),
            Ok(_) if url_has_password(&db.url) => report.problems.push(Problem::fatal(format!(
                "`{name}.url` must not contain a password; use `{name}.password`"
            ))),
            Ok(_) => {}
        }
    }
    if config.dev_mode {
        report
            .problems
            .push(Problem::config("development mode (`dev_mode`) is enabled"));
    }
    if !config.public_url.starts_with("https://") {
        report.problems.push(Problem::config(
            "`public_url` must use https (run behind an HTTPS reverse proxy)",
        ));
    }
    for (name, value) in [
        (
            "session.absolute_lifetime_seconds",
            config.session.absolute_lifetime_seconds,
        ),
        (
            "session.idle_timeout_seconds",
            config.session.idle_timeout_seconds,
        ),
        (
            "session.reauth_window_seconds",
            config.session.reauth_window_seconds,
        ),
        (
            "session.one_time_token_lifetime_seconds",
            config.session.one_time_token_lifetime_seconds,
        ),
        (
            "limits.login_attempts_per_ip_per_minute",
            config.limits.login_attempts_per_ip_per_minute,
        ),
        (
            "login_history.retention_seconds",
            config.login_history.retention_seconds,
        ),
        (
            "login_history.sweep_interval_seconds",
            config.login_history.sweep_interval_seconds,
        ),
    ] {
        if value == 0 {
            report
                .problems
                .push(Problem::fatal(format!("`{name}` must be greater than 0")));
        }
    }
    if !(1024 * 1024..=4 * 1024 * 1024 * 1024).contains(&config.scripts.memory_limit_bytes) {
        report.problems.push(Problem::fatal(
            "`scripts.memory_limit_bytes` must be between 1 MiB and 4 GiB",
        ));
    }
    if !(10..=60_000).contains(&config.scripts.time_limit_ms) {
        report.problems.push(Problem::fatal(
            "`scripts.time_limit_ms` must be between 10 and 60000",
        ));
    }
    if !(10..=240_000).contains(&config.scripts.async_time_limit_ms) {
        report.problems.push(Problem::fatal(
            "`scripts.async_time_limit_ms` must be between 10 and 240000",
        ));
    }
    if let Some(smtp) = &config.smtp {
        report.problems.extend(check_smtp(smtp));
    }
    if let Err(e) = crate::auth::passkeys::relying_party(&config.public_url, "check") {
        report.problems.push(Problem::fatal(format!("{e:#}")));
    }
    if let Some(path) = &config.login_history.geoip_database
        && let Err(e) = crate::geoip::GeoIp::open(path)
    {
        report.problems.push(Problem::fatal(format!(
            "`login_history.geoip_database` is not usable: {e:#}"
        )));
    }
    if let Some(b) = &config.auth.breached_passwords
        && !url::Url::parse(&b.base_url).is_ok_and(|u| matches!(u.scheme(), "https" | "http"))
    {
        report.problems.push(Problem::fatal(
            "`auth.breached_passwords.base_url` must be an http(s) URL",
        ));
    }
    if let Some(oidc) = &config.auth.oidc {
        report
            .problems
            .extend(check_secret("auth.oidc.client_secret", &oidc.client_secret));
        if !url::Url::parse(&oidc.issuer)
            .is_ok_and(|u| matches!(u.scheme(), "https" | "http") && u.host().is_some())
        {
            report
                .problems
                .push(Problem::fatal("`auth.oidc.issuer` must be an http(s) URL"));
        }
        if oidc.client_id.trim().is_empty() {
            report
                .problems
                .push(Problem::fatal("`auth.oidc.client_id` is empty"));
        }
        for d in &oidc.jit_domains {
            if d.is_empty() || d.contains('@') || d != &d.to_lowercase() {
                report.problems.push(Problem::fatal(format!(
                    "`auth.oidc.jit_domains`: `{d}` must be a lowercase domain name"
                )));
            }
        }
    }
    if let Some(dir) = &config.admin.dist_dir
        && let Err(e) =
            crate::http::admin_ui::AdminUi::load(dir, config.media.public_base_url.as_deref())
    {
        report.problems.push(Problem::fatal(format!(
            "`admin.dist_dir` is not usable: {e:#}"
        )));
    }
    if let Err(e) = crate::egress::Egress::from_config(&config.egress) {
        report.problems.push(Problem::fatal(format!("{e:#}")));
    }
    if config
        .egress
        .proxy
        .as_ref()
        .is_some_and(|p| p.connect_by == crate::config::ProxyConnectBy::Hostname)
    {
        // 検査の後に名前解決の結果が変わる攻撃を防げるかが、プロキシ次第になる（Q128）
        tracing::warn!(
            "`egress.proxy.connect_by = \"hostname\"`: the proxy resolves destinations itself; \
             protection against DNS rebinding now depends on the proxy"
        );
    }
    report
        .problems
        .extend(check_audit_export(&config.audit_export));
    if config.audit.retention_days < crate::migrate::MIN_AUDIT_RETENTION_DAYS {
        report.problems.push(Problem::fatal(format!(
            "`audit.retention_days` must be at least {}",
            crate::migrate::MIN_AUDIT_RETENTION_DAYS
        )));
    }
    match &config.encryption_key_file {
        None => report
            .problems
            .push(Problem::fatal("`encryption_key_file` is not set")),
        Some(path) => {
            if let Err(e) = KeyRing::load(path) {
                report.problems.push(Problem::fatal(format!(
                    "encryption key is not available: {e:#}"
                )));
            }
        }
    }
    report
}

/// 監査ログの外部への書き出しの検査。Webhook は https だけ（本番）。秘密のファイルは読めて、十分に強いこと。
fn check_audit_export(export: &crate::config::AuditExportConfig) -> Vec<Problem> {
    let mut problems = Vec::new();
    for (i, hook) in export.webhooks.iter().enumerate() {
        let name = format!("audit_export.webhooks[{i}]");
        match url::Url::parse(&hook.url) {
            Ok(u) if u.scheme() == "https" => {}
            Ok(u) if u.scheme() == "http" => problems.push(Problem::config(format!(
                "`{name}.url` is not https; audit records would be sent unencrypted"
            ))),
            _ => problems.push(Problem::fatal(format!(
                "`{name}.url` is not an http(s) URL"
            ))),
        }
        match std::fs::read_to_string(&hook.secret_file) {
            Ok(secret) => {
                problems.extend(check_secret(&format!("{name}.secret_file"), secret.trim()))
            }
            Err(_) => problems.push(Problem::fatal(format!(
                "`{name}.secret_file` cannot be read"
            ))),
        }
    }
    problems
}

/// SMTPの設定の検査。
fn check_smtp(smtp: &crate::config::SmtpConfig) -> Vec<Problem> {
    use crate::config::SmtpSecurity;
    let mut problems = Vec::new();
    if smtp.host.trim().is_empty() {
        problems.push(Problem::fatal("`smtp.host` is empty"));
    }
    if smtp.from.parse::<lettre::message::Mailbox>().is_err() {
        problems.push(Problem::fatal("`smtp.from` is not a valid email address"));
    }
    if smtp.username.is_some() != smtp.password.is_some() {
        problems.push(Problem::fatal(
            "`smtp.username` and `smtp.password` must be set together",
        ));
    }
    if let Some(password) = &smtp.password {
        problems.extend(check_secret("smtp.password", password));
    }
    let loopback = matches!(
        smtp.host.as_str(),
        "localhost" | "127.0.0.1" | "::1" | "[::1]"
    );
    if smtp.security == SmtpSecurity::None && !loopback {
        problems.push(Problem::config(
            "`smtp.security = \"none\"` sends mail unencrypted; use it only for a relay on this host",
        ));
    }
    problems
}

/// URLの userinfo にパスワードがあるか。
fn url_has_password(url: &str) -> bool {
    let Some(rest) = url.split_once("://").map(|(_, r)| r) else {
        return false;
    };
    let authority = rest.split(['/', '?']).next().unwrap_or("");
    match authority.rsplit_once('@') {
        Some((userinfo, _)) => userinfo.contains(':'),
        None => false,
    }
}

/// 接続したロールの属性。
#[derive(Debug)]
pub struct RoleInfo {
    pub server_version_num: i32,
    pub superuser: bool,
    pub create_role: bool,
}

pub async fn inspect_connection(db: &DatabaseConfig) -> Result<RoleInfo, sqlx::Error> {
    let options = crate::db::connect_options(db)?;
    let mut conn: PgConnection = tokio::time::timeout(Duration::from_secs(15), options.connect())
        .await
        .map_err(|_| sqlx::Error::PoolTimedOut)??;
    let server_version_num: String = sqlx::query_scalar("SHOW server_version_num")
        .fetch_one(&mut conn)
        .await?;
    let (superuser, create_role): (bool, bool) =
        sqlx::query_as("SELECT rolsuper, rolcreaterole FROM pg_roles WHERE rolname = current_user")
            .fetch_one(&mut conn)
            .await?;
    conn.close().await?;
    Ok(RoleInfo {
        server_version_num: server_version_num.trim().parse().unwrap_or(0),
        superuser,
        create_role,
    })
}

/// DBに接続して行う検査。
pub async fn check_databases(config: &Config) -> Report {
    let mut report = Report::default();
    for (name, db, needs_create_role) in [
        ("database", &config.database, false),
        ("migration_database", &config.migration_database, true),
    ] {
        match inspect_connection(db).await {
            Err(e) => report.problems.push(Problem::fatal(format!(
                "cannot connect with `{name}`: {}",
                describe_db_error(&e)
            ))),
            Ok(info) => {
                if info.server_version_num < MIN_POSTGRES_VERSION_NUM {
                    report.problems.push(Problem::fatal(format!(
                        "PostgreSQL 16 or later is required (`{name}` reports server_version_num {})",
                        info.server_version_num
                    )));
                }
                if info.superuser {
                    report
                        .problems
                        .push(Problem::config(format!("`{name}` connects as a superuser")));
                }
                if !needs_create_role && info.create_role {
                    report.problems.push(Problem::config(format!(
                        "`{name}` (the everyday connection) must not have CREATEROLE"
                    )));
                }
                if needs_create_role && !info.create_role && !info.superuser {
                    report.problems.push(Problem::config(format!(
                        "`{name}` needs CREATEROLE to create workspace roles"
                    )));
                }
            }
        }
    }
    report
}

/// DBのエラーを、秘密の値を含まない短い文にする。
fn describe_db_error(e: &sqlx::Error) -> String {
    match e {
        sqlx::Error::Database(db) => format!("database error {}", db.code().unwrap_or_default()),
        sqlx::Error::Io(_) => "connection failed".to_owned(),
        sqlx::Error::Tls(_) => "TLS error".to_owned(),
        sqlx::Error::PoolTimedOut => "timed out".to_owned(),
        sqlx::Error::Configuration(_) => "invalid connection settings".to_owned(),
        _ => "connection error".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets() {
        assert!(check_secret("x", "").is_some());
        assert!(check_secret("x", "abc123").is_some());
        assert!(check_secret("x", "ChangeMe").is_some());
        assert!(check_secret("x", "aaaaaaaaaaaaaaaaaaaa").is_some());
        assert!(check_secret("x", "kq3-Vd8_mZ2pL9xR4tY7").is_none());
        // 問題の文に値が含まれない
        let p = check_secret("database.password", "abc123").unwrap();
        assert!(!p.message.contains("abc123"));
    }

    #[test]
    fn smtp() {
        use crate::config::{SmtpConfig, SmtpSecurity};
        let ok = SmtpConfig {
            host: "smtp.example.com".into(),
            port: None,
            security: SmtpSecurity::Starttls,
            username: Some("cms".into()),
            password: Some("kq3-Vd8_mZ2pL9xR4tY7".into()),
            from: "CMS <cms@example.com>".into(),
        };
        assert!(check_smtp(&ok).is_empty());
        let plain_remote = SmtpConfig {
            security: SmtpSecurity::None,
            ..ok.clone()
        };
        assert_eq!(check_smtp(&plain_remote).len(), 1);
        let plain_local = SmtpConfig {
            host: "127.0.0.1".into(),
            username: None,
            password: None,
            ..plain_remote
        };
        assert!(check_smtp(&plain_local).is_empty());
        let weak = SmtpConfig {
            password: Some("secret".into()),
            ..ok.clone()
        };
        assert_eq!(check_smtp(&weak).len(), 1);
        let bad_from = SmtpConfig {
            from: "not an address".into(),
            ..ok
        };
        assert!(check_smtp(&bad_from).iter().any(|p| p.fatal));
    }

    #[test]
    fn password_in_url() {
        assert!(url_has_password("postgres://u:p@h/db"));
        assert!(!url_has_password("postgres://u@h/db"));
        assert!(!url_has_password("postgres://h:5432/db"));
        assert!(!url_has_password("postgres://u@h/db?x=a:b@c"));
    }
}
