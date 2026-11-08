//! DBの接続と、SQLの識別子の扱い。
//!
//! SQLの識別子（スキーマ名・ロール名・テーブル名など）は、必ず [`Ident`] を通して組み立てる
//! （CLAUDE.md の禁止事項）。値は必ずパラメーターで渡す。

use std::fmt;
use std::time::Duration;

use sqlx::ConnectOptions as _;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};

use crate::config::DatabaseConfig;

/// 検証済みのSQLの識別子。英小文字・数字・アンダースコアだけで、先頭は英小文字かアンダースコア、63文字以内。
/// 表示（`Display`）すると、二重引用符で囲んだ形になる。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Ident(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidIdent;

impl fmt::Display for InvalidIdent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid SQL identifier")
    }
}

impl std::error::Error for InvalidIdent {}

impl Ident {
    /// 識別子を検証する。規則に合わなければ拒否する（引用符で逃がして受け入れることはしない）。
    pub fn new(name: &str) -> Result<Self, InvalidIdent> {
        let mut chars = name.chars();
        let first_ok = matches!(chars.next(), Some('a'..='z' | '_'));
        let rest_ok = chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_'));
        if first_ok && rest_ok && name.len() <= 63 {
            Ok(Self(name.to_owned()))
        } else {
            Err(InvalidIdent)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Ident {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 規則により二重引用符を含まないので、そのまま囲めばよい
        write!(f, "\"{}\"", self.0)
    }
}

/// 検証済みの識別子（[`Ident`]）と定数だけを埋め込んで組み立てたSQLであることを示す。
/// 利用者の入力を埋め込んではいけない。値は必ずパラメーターで渡す。
pub fn trusted(sql: String) -> sqlx::AssertSqlSafe<String> {
    sqlx::AssertSqlSafe(sql)
}

/// `format!` と同じ書き方で、[`trusted`] なSQLを組み立てる。埋め込んでよいのは [`Ident`] と定数だけ。
#[macro_export]
macro_rules! sql {
    ($($t:tt)*) => { $crate::db::trusted(format!($($t)*)) };
}

/// 接続の設定。SQL文はログに出さない（本文や秘密の値が含まれうるため）。
pub fn connect_options(db: &DatabaseConfig) -> Result<PgConnectOptions, sqlx::Error> {
    let options: PgConnectOptions = db.url.parse()?;
    Ok(options
        .password(&db.password)
        .log_statements(tracing::log::LevelFilter::Off))
}

/// 普段の処理用の接続のプール。
pub async fn app_pool(db: &DatabaseConfig) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(16)
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(connect_options(db)?)
        .await
}

/// 移行・ロールの作成用の接続。1本だけにする（Q179）。
pub async fn migration_pool(db: &DatabaseConfig) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(30))
        .connect_with(connect_options(db)?)
        .await
}

/// 接続しているロールの名前。
pub async fn current_role(pool: &PgPool) -> Result<Ident, anyhow::Error> {
    let name: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(pool)
        .await?;
    Ident::new(&name)
        .map_err(|_| anyhow::anyhow!("the database role name must use only a-z, 0-9 and _"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_simple_names() {
        for ok in [
            "yomi",
            "yomi_app",
            "_x",
            "ws_0123456789abcdef0123456789abcdef",
            &"a".repeat(63),
        ] {
            assert!(Ident::new(ok).is_ok(), "{ok}");
        }
        assert_eq!(Ident::new("yomi_app").unwrap().to_string(), "\"yomi_app\"");
    }

    #[test]
    fn rejects_everything_else() {
        for bad in [
            "",
            "Yomi",
            "1yomi",
            "yomi-app",
            "yomi app",
            "yomi\"; drop",
            "yomi.app",
            "ｃｍｓ",
            "é",
            &"a".repeat(64),
        ] {
            assert!(Ident::new(bad).is_err(), "{bad:?}");
        }
    }
}
