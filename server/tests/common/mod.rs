//! 結合テストで共通に使う部品。
//!
//! 環境変数 `CMS_TEST_PG_SUPERUSER_URL` にスーパーユーザーの接続先を渡すと、本物のPostgreSQLを使うテストを実行する。
//! テストごとに使い捨てのロールとDBを作り、終わったら消す。
#![allow(dead_code)]

pub mod content;
pub mod mmdb;
pub mod server;
pub mod smtp;

use cms_server::config::DatabaseConfig;
use cms_server::db;
use sqlx::postgres::PgPoolOptions;
use sqlx::{Executor, PgPool};
use uuid::Uuid;

pub struct Scratch {
    pub superuser: PgPool,
    pub database: String,
    pub app_role: String,
    pub migrate_role: String,
    pub app: DatabaseConfig,
    pub migration: DatabaseConfig,
}

/// `postgres://user:pw@host:port/db?...` から `host:port` を取り出す。
fn host_port(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split(['/', '?']).next().unwrap_or("");
    authority
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(authority)
        .to_owned()
}

pub fn superuser_url() -> Option<String> {
    std::env::var("CMS_TEST_PG_SUPERUSER_URL")
        .ok()
        .filter(|s| !s.is_empty())
}

impl Scratch {
    pub async fn new(url: &str) -> Scratch {
        let superuser = PgPoolOptions::new()
            .max_connections(2)
            .connect(url)
            .await
            .expect("superuser connection");
        let tag = Uuid::new_v4().simple().to_string()[..12].to_owned();
        let (database, app_role, migrate_role) = (
            format!("t_{tag}"),
            format!("t_{tag}_app"),
            format!("t_{tag}_mig"),
        );
        let (app_pw, mig_pw) = (
            Uuid::new_v4().simple().to_string(),
            Uuid::new_v4().simple().to_string(),
        );
        for sql in [
            format!("CREATE ROLE {migrate_role} LOGIN CREATEROLE PASSWORD '{mig_pw}'"),
            format!("CREATE ROLE {app_role} LOGIN PASSWORD '{app_pw}'"),
            format!("CREATE DATABASE {database} OWNER {migrate_role}"),
            format!("REVOKE ALL ON DATABASE {database} FROM PUBLIC"),
            format!("GRANT CONNECT ON DATABASE {database} TO {app_role}"),
        ] {
            superuser
                .execute(db::trusted(sql))
                .await
                .expect("scratch setup");
        }
        let host = host_port(url);
        let make = |role: &str, pw: &str| DatabaseConfig {
            url: format!("postgres://{role}@{host}/{database}"),
            password: pw.to_owned(),
        };
        let app = make(&app_role, &app_pw);
        let migration = make(&migrate_role, &mig_pw);
        Scratch {
            superuser,
            database,
            app_role,
            migrate_role,
            app,
            migration,
        }
    }

    pub async fn drop(self) {
        let su = &self.superuser;
        let _ = su
            .execute(db::trusted(format!(
                "DROP DATABASE IF EXISTS {} WITH (FORCE)",
                self.database
            )))
            .await;
        let roles: Vec<String> = sqlx::query_scalar("SELECT rolname::text FROM pg_roles WHERE rolname LIKE 'cms_ws_%' AND pg_has_role($1, oid, 'MEMBER')")
            .bind(&self.migrate_role)
            .fetch_all(su)
            .await
            .unwrap_or_default();
        for r in roles {
            let _ = su
                .execute(db::trusted(format!("DROP ROLE IF EXISTS \"{r}\"")))
                .await;
        }
        // 監査ログの削除の専用のロール（移行が普段用のロールの名前から作る。migrate::audit_purger_role）
        let _ = su
            .execute(db::trusted(format!(
                "DROP ROLE IF EXISTS \"{}_audit_purger\"",
                self.app_role.trim_matches('"')
            )))
            .await;
        let _ = su
            .execute(db::trusted(format!(
                "DROP ROLE IF EXISTS {}",
                self.app_role
            )))
            .await;
        let _ = su
            .execute(db::trusted(format!(
                "DROP ROLE IF EXISTS {}",
                self.migrate_role
            )))
            .await;
    }
}

/// 使い捨てのDBを用意して `body` を実行し、失敗しても後片付けをしてから失敗を伝える。
/// `CMS_TEST_PG_SUPERUSER_URL` がなければ飛ばす。
pub async fn with_scratch<F, Fut>(body: F)
where
    F: FnOnce(DatabaseConfig, DatabaseConfig) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    with_scratch_at("CMS_TEST_PG_SUPERUSER_URL", &[], body).await
}

/// [`with_scratch`] と同じ。スーパーユーザーの接続先を環境変数 `var` から読み（なければ飛ばす）、
/// 使い捨てのDBに拡張 `extensions` をスーパーユーザーで作ってから `body` を実行する。
pub async fn with_scratch_at<F, Fut>(var: &str, extensions: &[&str], body: F)
where
    F: FnOnce(DatabaseConfig, DatabaseConfig) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let Some(url) = std::env::var(var).ok().filter(|s| !s.is_empty()) else {
        eprintln!("skipped: {var} is not set");
        return;
    };
    let s = Scratch::new(&url).await;
    if !extensions.is_empty() {
        let in_db = PgPoolOptions::new()
            .max_connections(1)
            .connect(&with_database(&url, &s.database))
            .await
            .expect("superuser connection to the scratch database");
        for ext in extensions {
            in_db
                .execute(db::trusted(format!("CREATE EXTENSION {ext}")))
                .await
                .expect("create extension");
        }
        in_db.close().await;
    }
    let outcome = tokio::spawn(body(s.app.clone(), s.migration.clone())).await;
    s.drop().await;
    if let Err(e) = outcome {
        std::panic::resume_unwind(e.into_panic());
    }
}

/// 接続先のURLのDB名を `database` にする。
fn with_database(url: &str, database: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("postgres", url));
    let (authority, tail) = match rest.find(['/', '?']) {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let query = tail.find('?').map_or("", |i| &tail[i..]);
    format!("{scheme}://{authority}/{database}{query}")
}
