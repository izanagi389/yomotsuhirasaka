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
    let Some(url) = superuser_url() else {
        eprintln!("skipped: CMS_TEST_PG_SUPERUSER_URL is not set");
        return;
    };
    let s = Scratch::new(&url).await;
    let outcome = tokio::spawn(body(s.app.clone(), s.migration.clone())).await;
    s.drop().await;
    if let Err(e) = outcome {
        std::panic::resume_unwind(e.into_panic());
    }
}
