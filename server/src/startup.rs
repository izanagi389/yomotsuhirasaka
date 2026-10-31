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
    report.problems.extend(check_audit_export(
        &config.audit_export,
        &config.egress.internal_destination_exceptions,
    ));
    if let Some(llm) = &config.llm {
        report.problems.extend(check_llm(
            llm,
            &config.egress.internal_destination_exceptions,
        ));
    }
    if config.search.related_batch_hour_utc > 23 {
        report.problems.push(Problem::fatal(
            "`search.related_batch_hour_utc` must be between 0 and 23",
        ));
    }
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

/// 監査ログの外部への書き出しの検査。Webhook は https だけ（本番）。ただし、外部への送信の例外として許した内部の宛先には
/// http も許す（Q139）。秘密のファイルは読めて、十分に強いこと。
fn check_audit_export(
    export: &crate::config::AuditExportConfig,
    exceptions: &[String],
) -> Vec<Problem> {
    let mut problems = Vec::new();
    for (i, hook) in export.webhooks.iter().enumerate() {
        let name = format!("audit_export.webhooks[{i}]");
        match url::Url::parse(&hook.url) {
            Ok(u) if u.scheme() == "https" => {}
            Ok(u)
                if u.host_str()
                    .is_some_and(|h| exceptions.iter().any(|e| e.eq_ignore_ascii_case(h))) => {}
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

/// LLMの接続先の検査。接続先は https だけ（本番）。ただし、外部への送信の例外として許した内部の宛先には
/// http も許す（Q139。擬似LLMやローカルのLLM）。APIキーは空にできない（値そのものは問題の文に含めない）。
fn check_llm(llm: &crate::config::LlmConfig, exceptions: &[String]) -> Vec<Problem> {
    let mut problems = check_endpoint("llm", &llm.base_url, &llm.api_key, exceptions);
    problems.extend(check_model(
        "llm",
        &llm.embedding_model,
        llm.embedding_dimensions,
        llm.embedding_price_per_million_tokens,
        ("embedding_model", "embedding_dimensions"),
    ));
    // 回答の生成のモデル（M4）
    if llm
        .chat_model
        .as_deref()
        .is_some_and(|m| m.trim().is_empty())
    {
        problems.push(Problem::fatal("`llm.chat_model` is empty"));
    }
    for (name, price) in [
        (
            "chat_input_price_per_million_tokens",
            llm.chat_input_price_per_million_tokens,
        ),
        (
            "chat_output_price_per_million_tokens",
            llm.chat_output_price_per_million_tokens,
        ),
    ] {
        if price.is_some_and(|p| !p.is_finite() || p < 0.0) {
            problems.push(Problem::fatal(format!(
                "`llm.{name}` must be a non-negative number"
            )));
        }
    }
    // 切り替え先のモデル（接続先とキーは、書いたものだけを検査する。省略すれば今のものを使う）
    if let Some(next) = &llm.next_embedding {
        const P: &str = "llm.next_embedding";
        match (&next.base_url, &next.api_key) {
            (Some(url), Some(key)) => problems.extend(check_endpoint(P, url, key, exceptions)),
            (Some(_), None) => problems.push(Problem::fatal(format!(
                "`{P}.api_key` is required when `{P}.base_url` is set \
                 (the current API key is never sent to another endpoint)"
            ))),
            (None, Some(key)) if key.trim().is_empty() => {
                problems.push(Problem::fatal(format!("`{P}.api_key` is empty")))
            }
            _ => {}
        }
        problems.extend(check_model(
            P,
            &next.model,
            next.dimensions,
            next.embedding_price_per_million_tokens,
            ("model", "dimensions"),
        ));
        if next.model == llm.embedding_model && next.dimensions == llm.embedding_dimensions {
            problems.push(Problem::fatal(format!(
                "`{P}` is the same model as `llm.embedding_model`"
            )));
        }
    }
    // ワークスペースが選べる接続先（名前は空でなく重ならない。URL は今の接続先と同じ規則。Q26）
    for (i, e) in llm.allowed_endpoints.iter().enumerate() {
        let prefix = format!("llm.allowed_endpoints[{i}]");
        if e.name.trim().is_empty() {
            problems.push(Problem::fatal(format!("`{prefix}.name` is empty")));
        }
        if llm.allowed_endpoints[..i].iter().any(|o| o.name == e.name) {
            problems.push(Problem::fatal(format!(
                "`{prefix}.name` is used by another allowed endpoint"
            )));
        }
        problems.extend(check_base_url(&prefix, &e.base_url, exceptions));
    }
    // ワークスペースが選べる埋め込みのモデル
    for (i, m) in llm.allowed_embedding_models.iter().enumerate() {
        let prefix = format!("llm.allowed_embedding_models[{i}]");
        problems.extend(check_model(
            &prefix,
            &m.model,
            m.dimensions,
            m.embedding_price_per_million_tokens,
            ("model", "dimensions"),
        ));
        // ワークスペースはモデルの名前で選ぶので、名前は重ならないようにする
        if llm.allowed_embedding_models[..i]
            .iter()
            .any(|o| o.model == m.model)
        {
            problems.push(Problem::fatal(format!(
                "`{prefix}.model` is listed more than once"
            )));
        }
    }
    problems
}

/// LLMの接続先とキーの検査（`prefix` は設定の場所。問題の文にキーの値を入れない）。
fn check_endpoint(
    prefix: &str,
    base_url: &str,
    api_key: &str,
    exceptions: &[String],
) -> Vec<Problem> {
    let mut problems = check_base_url(prefix, base_url, exceptions);
    if api_key.trim().is_empty() {
        problems.push(Problem::fatal(format!("`{prefix}.api_key` is empty")));
    }
    problems
}

/// LLMの接続先の URL の検査（本番では https。http は外部への送信の例外に入れたホストだけ）。
fn check_base_url(prefix: &str, base_url: &str, exceptions: &[String]) -> Vec<Problem> {
    let mut problems = Vec::new();
    match url::Url::parse(base_url) {
        Ok(u) if u.host_str().is_none() => {
            problems.push(Problem::fatal(format!("`{prefix}.base_url` has no host")))
        }
        Ok(u) if u.scheme() == "https" => {}
        Ok(u)
            if u.scheme() == "http"
                && u.host_str()
                    .is_some_and(|h| exceptions.iter().any(|e| e.eq_ignore_ascii_case(h))) => {}
        Ok(u) if u.scheme() == "http" => problems.push(Problem::config(format!(
            "`{prefix}.base_url` is not https; article text would be sent unencrypted \
             (http is allowed only for hosts in `egress.internal_destination_exceptions`)"
        ))),
        _ => problems.push(Problem::fatal(format!(
            "`{prefix}.base_url` is not an http(s) URL"
        ))),
    }
    problems
}

/// 埋め込みのモデル・次元数・単価の検査（`names` は設定の中のモデルと次元数の名前）。
fn check_model(
    prefix: &str,
    model: &str,
    dimensions: u32,
    price: Option<f64>,
    names: (&str, &str),
) -> Vec<Problem> {
    let mut problems = Vec::new();
    if model.trim().is_empty() {
        problems.push(Problem::fatal(format!("`{prefix}.{}` is empty", names.0)));
    }
    if !(1..=crate::config::MAX_EMBEDDING_DIMENSIONS).contains(&dimensions) {
        problems.push(Problem::fatal(format!(
            "`{prefix}.{}` must be between 1 and {}",
            names.1,
            crate::config::MAX_EMBEDDING_DIMENSIONS
        )));
    }
    if price.is_some_and(|p| !p.is_finite() || p < 0.0) {
        problems.push(Problem::fatal(format!(
            "`{prefix}.embedding_price_per_million_tokens` must be a non-negative number"
        )));
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
    if config.llm.is_some() {
        report
            .problems
            .extend(check_vector_extension(&config.database).await);
    }
    report
}

/// LLMの接続先（`[llm]`）があるときは、検索用データのベクトルに pgvector の拡張 `vector` が要る（08-search-and-related.md）。
/// 拡張はスーパーユーザーが前もって作る（CMSのロールはスーパーユーザーでないため、CMSは作らない）。
/// ワークスペースのロールも使えるよう、拡張のスキーマは PUBLIC が USAGE を持つこと（public スキーマなら、もとからそう）。
async fn check_vector_extension(db: &DatabaseConfig) -> Vec<Problem> {
    let checked = async {
        let options = crate::db::connect_options(db)?;
        let mut conn: PgConnection =
            tokio::time::timeout(Duration::from_secs(15), options.connect())
                .await
                .map_err(|_| anyhow::anyhow!("timed out"))??;
        let schema = crate::search::vector_schema(&mut conn).await?;
        let usable = match &schema {
            Some(s) => crate::search::usable_by_every_role(&mut conn, s).await?,
            None => false,
        };
        conn.close().await?;
        anyhow::Ok((schema, usable))
    }
    .await;
    match checked {
        Err(e) => vec![Problem::fatal(format!(
            "cannot check the PostgreSQL extension `vector`: {e}"
        ))],
        Ok((None, _)) => vec![Problem::fatal(
            "`[llm]` is set, but the PostgreSQL extension `vector` (pgvector) is not installed in the database. \
             Create it as a superuser: CREATE EXTENSION vector;",
        )],
        Ok((Some(schema), false)) => vec![Problem::fatal(format!(
            "the schema `{}` of the extension `vector` must be usable by every role: GRANT USAGE ON SCHEMA {} TO PUBLIC;",
            schema.as_str(),
            schema
        ))],
        Ok((Some(_), true)) => Vec::new(),
    }
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
    fn next_embedding() {
        use crate::config::{LlmConfig, NextEmbeddingConfig};
        let next = NextEmbeddingConfig {
            model: "text-embedding-3-large".into(),
            dimensions: 1024,
            base_url: None,
            api_key: None,
            embedding_price_per_million_tokens: None,
        };
        let base = LlmConfig {
            base_url: "https://api.example.com/v1".into(),
            api_key: "sk-kq3Vd8mZ2pL9xR4tY7".into(),
            embedding_model: "text-embedding-3-small".into(),
            embedding_dimensions: 1536,
            embedding_price_per_million_tokens: None,
            default_workspace_embedding_tokens_per_month: None,
            default_workspace_embedding_tokens_per_day: None,
            chat_model: None,
            chat_input_price_per_million_tokens: None,
            chat_output_price_per_million_tokens: None,
            default_workspace_answer_input_tokens_per_month: None,
            default_workspace_answer_input_tokens_per_day: None,
            default_workspace_answer_output_tokens_per_month: None,
            default_workspace_answer_output_tokens_per_day: None,
            default_workspace_assist_input_tokens_per_month: None,
            default_workspace_assist_input_tokens_per_day: None,
            default_workspace_assist_output_tokens_per_month: None,
            default_workspace_assist_output_tokens_per_day: None,
            next_embedding: Some(next.clone()),
            allowed_endpoints: Vec::new(),
            allowed_embedding_models: Vec::new(),
        };
        let none: &[String] = &[];
        assert!(check_llm(&base, none).is_empty());
        // 接続先とキーを省略すれば、今のものを使う
        let n = base.next().unwrap();
        assert_eq!(
            (n.base_url.as_str(), n.api_key.as_str()),
            (base.base_url.as_str(), base.api_key.as_str())
        );
        assert_eq!(
            (n.embedding_model.as_str(), n.embedding_dimensions),
            ("text-embedding-3-large", 1024)
        );
        assert!(base.for_model("text-embedding-3-small", 1536).is_some());
        assert!(base.for_model("text-embedding-3-large", 1024).is_some());
        assert!(base.for_model("text-embedding-3-large", 512).is_none());
        // 接続先だけを書いてキーがなければ、起動しない（今のキーを別の宛先に送らない）
        let url_only = LlmConfig {
            next_embedding: Some(NextEmbeddingConfig {
                base_url: Some("https://other.example.com/v1".into()),
                ..next.clone()
            }),
            ..base.clone()
        };
        let problems = check_llm(&url_only, none);
        assert!(problems.len() == 1 && problems[0].fatal, "{problems:?}");
        assert!(url_only.next().is_none());
        // 接続先とキーを書けば、それを使い、今の接続先と同じ検査をする
        let own = LlmConfig {
            next_embedding: Some(NextEmbeddingConfig {
                base_url: Some("http://mock-llm:8080/v1".into()),
                api_key: Some("other-key".into()),
                ..next.clone()
            }),
            ..base.clone()
        };
        assert_eq!(own.next().unwrap().api_key, "other-key");
        assert_eq!(check_llm(&own, none).len(), 1, "http needs an exception");
        assert!(check_llm(&own, &["mock-llm".to_owned()]).is_empty());
        // 致命的なもの
        for bad in [
            NextEmbeddingConfig {
                api_key: Some(" ".into()),
                ..next.clone()
            },
            NextEmbeddingConfig {
                model: "".into(),
                ..next.clone()
            },
            NextEmbeddingConfig {
                dimensions: 0,
                ..next.clone()
            },
            NextEmbeddingConfig {
                model: "text-embedding-3-small".into(),
                dimensions: 1536,
                ..next.clone()
            },
        ] {
            let c = LlmConfig {
                next_embedding: Some(bad),
                ..base.clone()
            };
            let problems = check_llm(&c, none);
            assert!(problems.len() == 1 && problems[0].fatal, "{problems:?}");
        }
        // キーの値は Debug の出力に含まれない
        assert!(!format!("{own:?}").contains("other-key"));
    }

    #[test]
    fn llm() {
        use crate::config::LlmConfig;
        let ok = LlmConfig {
            base_url: "https://api.example.com/v1".into(),
            api_key: "sk-kq3Vd8mZ2pL9xR4tY7".into(),
            embedding_model: "text-embedding-3-small".into(),
            embedding_dimensions: 1536,
            embedding_price_per_million_tokens: Some(0.02),
            default_workspace_embedding_tokens_per_month: None,
            default_workspace_embedding_tokens_per_day: None,
            chat_model: None,
            chat_input_price_per_million_tokens: None,
            chat_output_price_per_million_tokens: None,
            default_workspace_answer_input_tokens_per_month: None,
            default_workspace_answer_input_tokens_per_day: None,
            default_workspace_answer_output_tokens_per_month: None,
            default_workspace_answer_output_tokens_per_day: None,
            default_workspace_assist_input_tokens_per_month: None,
            default_workspace_assist_input_tokens_per_day: None,
            default_workspace_assist_output_tokens_per_month: None,
            default_workspace_assist_output_tokens_per_day: None,
            next_embedding: None,
            allowed_endpoints: Vec::new(),
            allowed_embedding_models: Vec::new(),
        };
        let none: &[String] = &[];
        assert!(check_llm(&ok, none).is_empty());
        // http は、外部への送信の例外に入れたホストだけ
        let mock = LlmConfig {
            base_url: "http://mock-llm:8080/v1".into(),
            ..ok.clone()
        };
        let problems = check_llm(&mock, none);
        assert_eq!(problems.len(), 1);
        assert!(!problems[0].fatal, "本番の設定でだけ起動を止める");
        assert!(check_llm(&mock, &["MOCK-LLM".to_owned()]).is_empty());
        assert!(check_llm(&mock, &["other".to_owned()]).len() == 1);
        // 致命的なもの（設定によらず起動を止める）
        for bad in [
            LlmConfig {
                base_url: "ftp://x/v1".into(),
                ..ok.clone()
            },
            LlmConfig {
                api_key: "  ".into(),
                ..ok.clone()
            },
            LlmConfig {
                embedding_model: "".into(),
                ..ok.clone()
            },
            LlmConfig {
                embedding_dimensions: 0,
                ..ok.clone()
            },
            LlmConfig {
                embedding_dimensions: 2001,
                ..ok.clone()
            },
            LlmConfig {
                embedding_price_per_million_tokens: Some(-1.0),
                ..ok.clone()
            },
        ] {
            let problems = check_llm(&bad, none);
            assert_eq!(problems.len(), 1, "{bad:?}");
            assert!(problems[0].fatal, "{bad:?}");
        }
        // キーの値は、問題の文にも Debug の出力にも含まれない
        let key = "sk-very-secret-value-1234";
        let with_key = LlmConfig {
            api_key: key.into(),
            base_url: "nope".into(),
            ..ok.clone()
        };
        assert!(
            check_llm(&with_key, none)
                .iter()
                .all(|p| !p.message.contains(key))
        );
        assert!(!format!("{with_key:?}").contains(key));
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
