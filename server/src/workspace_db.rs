//! ワークスペースごとのDBのスキーマとロール（Q92, Q111, Q120）。
//!
//! - ワークスペースごとに、スキーマ `ws_<ID>` とロール `cms_ws_<ID>`（ログイン不可）を作る。
//! - ロールは自分のスキーマにだけ権限を持ち、共通のスキーマ（`cms`）や他のワークスペースには触れられない。
//! - 普段用のロールは、ワークスペースのロールに切り替える権限（SET）だけを持ち、その権限を引き継がない（INHERIT FALSE）。
//! - ワークスペースの処理は、[`begin`] で始めたトランザクションの中で、そのロールに切り替えて行う。
//!
//! DBのロールが防ぐのは「切り替え忘れ」などの不具合で、SQLインジェクションではない（13-security.md）。

use anyhow::{Context, Result, bail};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::Ident;

/// ワークスペースのロールで実行する文の時間の上限（Q120）。
/// ロールに ALTER ROLE ... SET を付けても SET ROLE では効かないため、トランザクションごとに設定する。
pub const STATEMENT_TIMEOUT: &str = "30s";

/// ワークスペースのDBの名前。
#[derive(Debug, Clone)]
pub struct WorkspaceDb {
    pub workspace_id: Uuid,
    pub schema: Ident,
    pub role: Ident,
}

impl WorkspaceDb {
    pub fn names_for(workspace_id: Uuid) -> Self {
        let hex = workspace_id.simple().to_string();
        Self {
            workspace_id,
            schema: Ident::new(&format!("ws_{hex}")).expect("uuid hex is a valid identifier"),
            role: Ident::new(&format!("cms_ws_{hex}")).expect("uuid hex is a valid identifier"),
        }
    }
}

/// ワークスペースを登録し、そのスキーマとロールを作る。移行・ロールの作成用の接続で呼ぶ（Q173, Q179）。
pub async fn provision(
    migration: &PgPool,
    app_role: &Ident,
    workspace_id: Uuid,
    name: &str,
    template: &str,
) -> Result<WorkspaceDb> {
    let db = WorkspaceDb::names_for(workspace_id);
    let (schema, role) = (&db.schema, &db.role);
    let mut tx = migration.begin().await?;
    for sql in [
        format!("CREATE ROLE {role} NOLOGIN NOINHERIT"),
        format!("CREATE SCHEMA {schema}"),
        format!("REVOKE ALL ON SCHEMA {schema} FROM PUBLIC"),
        format!("GRANT USAGE ON SCHEMA {schema} TO {role}"),
        // 移行で作るテーブルとシーケンスには、ワークスペースのロールの読み書きの権限を自動で付ける（DDLの権限は付けない）
        format!(
            "ALTER DEFAULT PRIVILEGES IN SCHEMA {schema} GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO {role}"
        ),
        format!(
            "ALTER DEFAULT PRIVILEGES IN SCHEMA {schema} GRANT USAGE, SELECT ON SEQUENCES TO {role}"
        ),
        format!("GRANT {role} TO {app_role} WITH INHERIT FALSE, SET TRUE"),
    ] {
        sqlx::query(crate::db::trusted(sql))
            .execute(&mut *tx)
            .await
            .with_context(|| format!("cannot provision workspace {workspace_id}"))?;
    }
    sqlx::query("INSERT INTO cms.workspaces (id, name, template, db_schema, db_role) VALUES ($1, $2, $3, $4, $5)")
        .bind(workspace_id)
        .bind(name)
        .bind(template)
        .bind(schema.as_str())
        .bind(role.as_str())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(db)
}

/// ワークスペースのDBの名前を、共通のスキーマから引く（普段用のロールで呼ぶ）。
pub async fn lookup(app: &PgPool, workspace_id: Uuid) -> Result<Option<WorkspaceDb>> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT db_schema, db_role FROM cms.workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_optional(app)
            .await?;
    let Some((schema, role)) = row else {
        return Ok(None);
    };
    let db = WorkspaceDb {
        workspace_id,
        schema: Ident::new(&schema)?,
        role: Ident::new(&role)?,
    };
    // 登録された名前が、IDから決まる名前と一致することを確かめる（取り違えの防止）
    let expected = WorkspaceDb::names_for(workspace_id);
    if db.schema != expected.schema || db.role != expected.role {
        bail!("workspace {workspace_id} has unexpected database names");
    }
    Ok(Some(db))
}

/// ワークスペースのロールに切り替えたトランザクションを始める。
/// このトランザクションの中では、そのワークスペースのスキーマのテーブルにしか触れられない。
pub async fn begin<'a>(app: &'a PgPool, db: &WorkspaceDb) -> Result<Transaction<'a, Postgres>> {
    let mut tx = app.begin().await?;
    let (schema, role) = (&db.schema, &db.role);
    sqlx::query(crate::sql!("SET LOCAL ROLE {role}"))
        .execute(&mut *tx)
        .await?;
    sqlx::query(crate::sql!("SET LOCAL search_path TO {schema}"))
        .execute(&mut *tx)
        .await?;
    sqlx::query(crate::sql!(
        "SET LOCAL statement_timeout TO '{STATEMENT_TIMEOUT}'"
    ))
    .execute(&mut *tx)
    .await?;
    Ok(tx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_derived_from_the_id() {
        let id = Uuid::parse_str("0123456789abcdef0123456789abcdef").unwrap();
        let db = WorkspaceDb::names_for(id);
        assert_eq!(db.schema.as_str(), "ws_0123456789abcdef0123456789abcdef");
        assert_eq!(db.role.as_str(), "cms_ws_0123456789abcdef0123456789abcdef");
    }
}
