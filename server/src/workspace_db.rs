//! ワークスペースごとのDBのスキーマとロール（Q92, Q111, Q120）。
//!
//! - ワークスペースごとに、スキーマ `ws_<ID>` とロール `yomi_ws_<ID>`（ログイン不可）を作る。
//! - ロールは自分のスキーマにだけ権限を持ち、共通のスキーマ（`yomi`）や他のワークスペースには触れられない。
//! - 普段用のロールは、ワークスペースのロールに切り替える権限（SET）だけを持ち、その権限を引き継がない（INHERIT FALSE）。
//! - ワークスペースの処理は、[`begin`] で始めたトランザクションの中で、そのロールに切り替えて行う。
//!
//! DBのロールが防ぐのは「切り替え忘れ」などの不具合で、SQLインジェクションではない（13-security.md）。

use anyhow::{Context, Result, bail};
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
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
            role: Ident::new(&format!("yomi_ws_{hex}")).expect("uuid hex is a valid identifier"),
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
    sqlx::query("INSERT INTO yomi.workspaces (id, name, template, db_schema, db_role) VALUES ($1, $2, $3, $4, $5)")
        .bind(workspace_id)
        .bind(name)
        .bind(template)
        .bind(schema.as_str())
        .bind(role.as_str())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let mut conn = migration.acquire().await?;
    upgrade_base(&mut conn, &db).await?;
    crate::search::ensure_keyword_index(&mut conn, &db).await?;
    Ok(db)
}

/// ワークスペースを完全に削除する（11-permissions-and-audit.md「ワークスペースと利用者」、INV-TEN-08、INV-DB-05）。
/// 移行・ロールの作成用の接続の中で呼ぶ（デプロイのロックを持って）。消えたら true、なければ false。
///
/// 1つのトランザクションで、スキーマ（記事・版・メディアの記録など）とロールを消し、個人トークンからそのワークスペースを外し、
/// 共通のテーブルの行を消す（所属・招待・APIキー・Webhook・ジョブ・設定は `ON DELETE CASCADE` で消える）。
/// 監査ログの行は消さない（通常の保存期間まで残す）。メディアのファイルは呼ぶ側が消す。
pub async fn destroy(conn: &mut PgConnection, workspace_id: Uuid) -> Result<bool> {
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    let found: Option<(String, String)> =
        sqlx::query_as("SELECT db_schema, db_role FROM yomi.workspaces WHERE id = $1 FOR UPDATE")
            .bind(workspace_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((schema, role)) = found else {
        return Ok(false);
    };
    // 登録された名前が、IDから決まる名前と一致することを確かめる（取り違えて別のスキーマを消さない）
    let expected = WorkspaceDb::names_for(workspace_id);
    if schema != expected.schema.as_str() || role != expected.role.as_str() {
        bail!("workspace {workspace_id} has unexpected database names");
    }
    let (schema, role) = (&expected.schema, &expected.role);
    sqlx::query(crate::sql!("DROP SCHEMA {schema} CASCADE"))
        .execute(&mut *tx)
        .await
        .with_context(|| format!("cannot drop the schema of workspace {workspace_id}"))?;
    sqlx::query(crate::sql!("DROP ROLE {role}"))
        .execute(&mut *tx)
        .await
        .with_context(|| format!("cannot drop the role of workspace {workspace_id}"))?;
    sqlx::query(
        "UPDATE yomi.personal_tokens SET workspace_ids = array_remove(workspace_ids, $1)
         WHERE $1 = ANY (workspace_ids)",
    )
    .bind(workspace_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM yomi.workspaces WHERE id = $1")
        .bind(workspace_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

/// ワークスペースのスキーマの土台のテーブルの移行（版, SQL）。版の昇順に並べる。
/// SQLは、search_path をそのワークスペースのスキーマにした中で実行する。
pub const BASE_MIGRATIONS: &[(i32, &str)] = &[
    (1, include_str!("../migrations/workspace/0001_entries.sql")),
    (
        2,
        include_str!("../migrations/workspace/0002_publishing.sql"),
    ),
    (
        3,
        include_str!("../migrations/workspace/0003_translations.sql"),
    ),
    (4, include_str!("../migrations/workspace/0004_media.sql")),
    (5, include_str!("../migrations/workspace/0005_queries.sql")),
    (
        6,
        include_str!("../migrations/workspace/0006_publish_settings.sql"),
    ),
    (7, include_str!("../migrations/workspace/0007_rendered.sql")),
    (
        8,
        include_str!("../migrations/workspace/0008_url_dependencies.sql"),
    ),
    (
        9,
        include_str!("../migrations/workspace/0009_corrections_freshness.sql"),
    ),
    (
        10,
        include_str!("../migrations/workspace/0010_review_requests.sql"),
    ),
    (
        11,
        include_str!("../migrations/workspace/0011_schedules.sql"),
    ),
    (
        12,
        include_str!("../migrations/workspace/0012_edit_locks.sql"),
    ),
    (
        13,
        include_str!("../migrations/workspace/0013_preview_tokens.sql"),
    ),
    (
        14,
        include_str!("../migrations/workspace/0014_search_chunks.sql"),
    ),
    (
        15,
        include_str!("../migrations/workspace/0015_search_fields_key.sql"),
    ),
    (
        16,
        include_str!("../migrations/workspace/0016_search_chunks_without_embedding.sql"),
    ),
    (
        17,
        include_str!("../migrations/workspace/0017_search_embeddings.sql"),
    ),
    (
        18,
        include_str!("../migrations/workspace/0018_search_models.sql"),
    ),
    (
        19,
        include_str!("../migrations/workspace/0019_related_entries.sql"),
    ),
    (
        20,
        include_str!("../migrations/workspace/0020_answer_phrases.sql"),
    ),
    (
        21,
        include_str!("../migrations/workspace/0021_answer_cache.sql"),
    ),
    (
        22,
        include_str!("../migrations/workspace/0022_answer_settings.sql"),
    ),
    (
        23,
        include_str!("../migrations/workspace/0023_question_log.sql"),
    ),
    (
        24,
        include_str!("../migrations/workspace/0024_answer_refusal.sql"),
    ),
    (
        25,
        include_str!("../migrations/workspace/0025_assist_settings.sql"),
    ),
    (
        26,
        include_str!("../migrations/workspace/0026_assist_conflicts.sql"),
    ),
    (
        27,
        include_str!("../migrations/workspace/0027_assist_tasks.sql"),
    ),
];

/// 1つのワークスペースの土台のテーブルを、最新の版にそろえる。移行・ロールの作成用の接続で呼ぶ。
pub async fn upgrade_base(conn: &mut PgConnection, db: &WorkspaceDb) -> Result<()> {
    let schema = &db.schema;
    for &(version, sql) in BASE_MIGRATIONS {
        let mut tx = sqlx::Connection::begin(&mut *conn).await?;
        let current: i32 =
            sqlx::query_scalar("SELECT base_version FROM yomi.workspaces WHERE id = $1 FOR UPDATE")
                .bind(db.workspace_id)
                .fetch_one(&mut *tx)
                .await?;
        if current >= version {
            continue;
        }
        sqlx::query(crate::sql!("SET LOCAL search_path TO {schema}"))
            .execute(&mut *tx)
            .await?;
        sqlx::raw_sql(sql)
            .execute(&mut *tx)
            .await
            .with_context(|| {
                format!(
                    "workspace {} base migration {version} failed",
                    db.workspace_id
                )
            })?;
        sqlx::query("UPDATE yomi.workspaces SET base_version = $2 WHERE id = $1")
            .bind(db.workspace_id)
            .bind(version)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }
    Ok(())
}

/// すべてのワークスペースの土台のテーブルをそろえる（サーバーの起動時）。
pub async fn upgrade_all(migration: &PgPool) -> Result<()> {
    let latest = BASE_MIGRATIONS.last().map_or(0, |m| m.0);
    let mut conn = migration.acquire().await?;
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM yomi.workspaces WHERE base_version < $1 ORDER BY created_at",
    )
    .bind(latest)
    .fetch_all(&mut *conn)
    .await?;
    for id in ids {
        upgrade_base(&mut conn, &WorkspaceDb::names_for(id)).await?;
    }
    Ok(())
}

/// ワークスペースのDBの名前を、共通のスキーマから引く（普段用のロールで呼ぶ）。
pub async fn lookup(app: &PgPool, workspace_id: Uuid) -> Result<Option<WorkspaceDb>> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT db_schema, db_role FROM yomi.workspaces WHERE id = $1")
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
    begin_owned(app, db).await
}

/// [`begin`] と同じ。プールを借りずに持てるトランザクションを返す（配信APIのリゾルバーから共有するため）。
pub async fn begin_owned(app: &PgPool, db: &WorkspaceDb) -> Result<Transaction<'static, Postgres>> {
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

/// ワークスペースのロールのトランザクションの中で、監査ログを書く。監査ログは共通のスキーマにあり、ワークスペースのロールは
/// 書けないので、普段用のロールに戻して書き、ワークスペースのロールに戻す。操作と同じトランザクションなので、操作が確定した
/// のに記録がない、ということが起きない（第三者の確認の R34）。
#[allow(clippy::too_many_arguments)]
pub async fn audit_full(
    tx: &mut Transaction<'_, Postgres>,
    db: &WorkspaceDb,
    workspace_id: Option<Uuid>,
    actor_user_id: Option<Uuid>,
    action: &str,
    target_ids: &[Uuid],
    notify: bool,
    details: Option<&serde_json::Value>,
) -> Result<()> {
    sqlx::query("RESET ROLE").execute(&mut **tx).await?;
    crate::audit::record_full(
        &mut **tx,
        workspace_id,
        actor_user_id,
        action,
        target_ids,
        notify,
        details,
    )
    .await?;
    let role = &db.role;
    sqlx::query(crate::sql!("SET LOCAL ROLE {role}"))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// [`audit_full`] の、詳細がない形。
pub async fn audit_notify(
    tx: &mut Transaction<'_, Postgres>,
    db: &WorkspaceDb,
    workspace_id: Option<Uuid>,
    actor_user_id: Option<Uuid>,
    action: &str,
    target_ids: &[Uuid],
    notify: bool,
) -> Result<()> {
    audit_full(
        tx,
        db,
        workspace_id,
        actor_user_id,
        action,
        target_ids,
        notify,
        None,
    )
    .await
}

/// [`audit_full`] の、通知も詳細もない形。
pub async fn audit(
    tx: &mut Transaction<'_, Postgres>,
    db: &WorkspaceDb,
    workspace_id: Option<Uuid>,
    actor_user_id: Option<Uuid>,
    action: &str,
    target_ids: &[Uuid],
) -> Result<()> {
    audit_full(
        tx,
        db,
        workspace_id,
        actor_user_id,
        action,
        target_ids,
        false,
        None,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_derived_from_the_id() {
        let id = Uuid::parse_str("0123456789abcdef0123456789abcdef").unwrap();
        let db = WorkspaceDb::names_for(id);
        assert_eq!(db.schema.as_str(), "ws_0123456789abcdef0123456789abcdef");
        assert_eq!(db.role.as_str(), "yomi_ws_0123456789abcdef0123456789abcdef");
    }
}
