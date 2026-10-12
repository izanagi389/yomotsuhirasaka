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
            "limits.login_attempts_per_ip_per_minute",
            config.limits.login_attempts_per_ip_per_minute,
        ),
    ] {
        if value == 0 {
            report
                .problems
                .push(Problem::fatal(format!("`{name}` must be greater than 0")));
        }
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
    fn password_in_url() {
        assert!(url_has_password("postgres://u:p@h/db"));
        assert!(!url_has_password("postgres://u@h/db"));
        assert!(!url_has_password("postgres://h:5432/db"));
        assert!(!url_has_password("postgres://u@h/db?x=a:b@c"));
    }
}
