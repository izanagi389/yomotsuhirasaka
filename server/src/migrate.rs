//! 共通のスキーマ（`cms`）の移行と、普段用のロールへの権限の付与。
//!
//! - 移行は、移行・ロールの作成用の接続（Q173）で行う。普段用のロールはDDLができない（INV-DB-02, INV-CFG-03）。
//! - 移行のSQLはバイナリに埋め込み、`cms.schema_migrations` に適用済みのバージョンを記録する。
//! - 権限は [`TABLE_PRIVILEGES`] に表として持ち、表にないテーブルがあれば起動を止める（付け忘れを防ぐ）。

use anyhow::{Context, Result, bail};
use sqlx::PgPool;

use crate::db::Ident;

/// 埋め込んだ移行（バージョン, 名前, SQL）。バージョンの昇順に並べる。
pub const MIGRATIONS: &[(i32, &str, &str)] = &[
    (1, "init", include_str!("../migrations/0001_init.sql")),
    (
        2,
        "users_sessions",
        include_str!("../migrations/0002_users_sessions.sql"),
    ),
    (3, "mfa", include_str!("../migrations/0003_mfa.sql")),
    (
        4,
        "memberships",
        include_str!("../migrations/0004_memberships.sql"),
    ),
    (5, "jobs", include_str!("../migrations/0005_jobs.sql")),
    (
        6,
        "invitations",
        include_str!("../migrations/0006_invitations.sql"),
    ),
    (
        7,
        "user_locale",
        include_str!("../migrations/0007_user_locale.sql"),
    ),
    (
        8,
        "reset_tokens",
        include_str!("../migrations/0008_reset_tokens.sql"),
    ),
    (
        9,
        "personal_tokens",
        include_str!("../migrations/0009_personal_tokens.sql"),
    ),
    (
        10,
        "api_keys",
        include_str!("../migrations/0010_api_keys.sql"),
    ),
    (
        11,
        "login_history",
        include_str!("../migrations/0011_login_history.sql"),
    ),
    (
        12,
        "passkeys",
        include_str!("../migrations/0012_passkeys.sql"),
    ),
    (
        13,
        "templates",
        include_str!("../migrations/0013_templates.sql"),
    ),
    (
        14,
        "workspace_limits",
        include_str!("../migrations/0014_workspace_limits.sql"),
    ),
    (
        15,
        "webhooks",
        include_str!("../migrations/0015_webhooks.sql"),
    ),
    (
        16,
        "external_identities",
        include_str!("../migrations/0016_external_identities.sql"),
    ),
    (17, "oidc", include_str!("../migrations/0017_oidc.sql")),
    (
        18,
        "job_dedupe",
        include_str!("../migrations/0018_job_dedupe.sql"),
    ),
    (
        19,
        "member_grants",
        include_str!("../migrations/0019_member_grants.sql"),
    ),
    (
        20,
        "display_names",
        include_str!("../migrations/0020_display_names.sql"),
    ),
    (
        21,
        "take_over_policy",
        include_str!("../migrations/0021_take_over_policy.sql"),
    ),
    (
        22,
        "preview_origins",
        include_str!("../migrations/0022_preview_origins.sql"),
    ),
    (
        23,
        "preview_path",
        include_str!("../migrations/0023_preview_path.sql"),
    ),
    (
        24,
        "audit_export",
        include_str!("../migrations/0024_audit_export.sql"),
    ),
];

/// 共通のスキーマの名前。
pub const SHARED_SCHEMA: &str = "cms";

/// 普段用のロールに付ける、共通のスキーマのテーブルごとの権限。
/// 監査ログは SELECT と INSERT だけ（更新・削除はできない。保存期間を過ぎたものの削除は専用のロールで行う）。
pub const TABLE_PRIVILEGES: &[(&str, &str)] = &[
    ("schema_migrations", "SELECT"),
    ("workspaces", "SELECT"),
    ("audit_log", "SELECT, INSERT"),
    ("users", "SELECT, INSERT, UPDATE"),
    ("sessions", "SELECT, INSERT, UPDATE"),
    // 作り直すときに古いコードを消す
    ("recovery_codes", "SELECT, INSERT, UPDATE, DELETE"),
    ("memberships", "SELECT, INSERT, UPDATE, DELETE"),
    // 終わったジョブを消す
    ("jobs", "SELECT, INSERT, UPDATE, DELETE"),
    ("invitations", "SELECT, INSERT, UPDATE"),
    ("reset_tokens", "SELECT, INSERT, UPDATE"),
    ("personal_tokens", "SELECT, INSERT, UPDATE"),
    ("api_keys", "SELECT, INSERT, UPDATE"),
    // 長く使われていない端末と、保存期間を過ぎた履歴を消す
    ("known_devices", "SELECT, INSERT, UPDATE, DELETE"),
    ("login_history", "SELECT, INSERT, DELETE"),
    ("passkeys", "SELECT, INSERT, UPDATE, DELETE"),
    // 同じセッション・用途で始め直したら上書きし、使ったら・期限を過ぎたら消す
    ("webauthn_challenges", "SELECT, INSERT, UPDATE, DELETE"),
    // 書き込みはデプロイの処理（移行・ロールの作成用の接続）だけ
    ("template_versions", "SELECT"),
    // サーバー管理者が上限を変える
    ("workspace_limits", "SELECT, INSERT, UPDATE"),
    // 管理者が設定を変える（編集の引き継ぎを誰ができるか）
    ("workspace_settings", "SELECT, INSERT, UPDATE"),
    // 管理者が削除する（送信の記録も一緒に消える）
    ("webhooks", "SELECT, INSERT, UPDATE, DELETE"),
    ("audit_sinks", "SELECT, INSERT, UPDATE, DELETE"),
    ("audit_export_cursors", "SELECT, INSERT, UPDATE, DELETE"),
    ("webhook_deliveries", "SELECT, INSERT, UPDATE"),
    // 本人がひも付けを外せるようにする（OIDCとともに）
    ("external_identities", "SELECT, INSERT, UPDATE, DELETE"),
    // OIDCのログインの途中の状態。使ったら・期限を過ぎたら消す
    ("oidc_logins", "SELECT, INSERT, DELETE"),
];

/// 移行の排他に使う、アドバイザリーロックのキー（任意の固定値）。
const MIGRATION_LOCK_KEY: i64 = 0x636d_735f_6d69_6772; // "cms_migr"

/// 未適用の移行を適用する。複数のプロセスが同時に呼んでも、1つずつ順に適用される。
pub async fn run(migration: &PgPool) -> Result<Vec<i32>> {
    let mut conn = migration.acquire().await?;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(MIGRATION_LOCK_KEY)
        .execute(&mut *conn)
        .await?;
    let result = run_locked(&mut conn).await;
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(MIGRATION_LOCK_KEY)
        .execute(&mut *conn)
        .await?;
    result
}

async fn run_locked(conn: &mut sqlx::PgConnection) -> Result<Vec<i32>> {
    let schema = Ident::new(SHARED_SCHEMA)?;
    sqlx::query(crate::sql!("CREATE SCHEMA IF NOT EXISTS {schema}"))
        .execute(&mut *conn)
        .await?;
    sqlx::query(crate::sql!(
        "CREATE TABLE IF NOT EXISTS {schema}.schema_migrations (version integer PRIMARY KEY, name text NOT NULL, applied_at timestamptz NOT NULL DEFAULT now())"
    ))
    .execute(&mut *conn)
    .await?;

    let applied: Vec<i32> = sqlx::query_scalar(crate::sql!(
        "SELECT version FROM {schema}.schema_migrations ORDER BY version"
    ))
    .fetch_all(&mut *conn)
    .await?;
    if let Some(&latest) = applied.last()
        && latest > MIGRATIONS.last().map(|m| m.0).unwrap_or(0)
    {
        bail!(
            "the database schema (version {latest}) is newer than this server; upgrade the server"
        );
    }

    let mut newly = Vec::new();
    for &(version, name, sql) in MIGRATIONS {
        if applied.contains(&version) {
            continue;
        }
        let mut tx = sqlx::Connection::begin(&mut *conn).await?;
        sqlx::raw_sql(sql)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("migration {version} ({name}) failed"))?;
        sqlx::query(crate::sql!(
            "INSERT INTO {schema}.schema_migrations (version, name) VALUES ($1, $2)"
        ))
        .bind(version)
        .bind(name)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        newly.push(version);
    }
    Ok(newly)
}

/// 普段用のロールに、共通のスキーマへの権限を付け直す（何度実行しても同じ結果になる）。
/// 表にないテーブルが共通のスキーマにあれば、エラーにする。
pub async fn apply_grants(migration: &PgPool, app_role: &Ident) -> Result<()> {
    let schema = Ident::new(SHARED_SCHEMA)?;
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT tablename::text FROM pg_tables WHERE schemaname = $1 ORDER BY 1",
    )
    .bind(SHARED_SCHEMA)
    .fetch_all(migration)
    .await?;
    let unknown: Vec<&String> = tables
        .iter()
        .filter(|t| !TABLE_PRIVILEGES.iter().any(|(name, _)| name == t))
        .collect();
    if !unknown.is_empty() {
        bail!("tables without a privilege policy in schema {SHARED_SCHEMA}: {unknown:?}");
    }

    let mut tx = migration.begin().await?;
    sqlx::query(crate::sql!("REVOKE ALL ON SCHEMA {schema} FROM PUBLIC"))
        .execute(&mut *tx)
        .await?;
    sqlx::query(crate::sql!("GRANT USAGE ON SCHEMA {schema} TO {app_role}"))
        .execute(&mut *tx)
        .await?;
    for (table, privileges) in TABLE_PRIVILEGES {
        let table = Ident::new(table)?;
        sqlx::query(crate::sql!(
            "REVOKE ALL ON {schema}.{table} FROM PUBLIC, {app_role}"
        ))
        .execute(&mut *tx)
        .await?;
        // privileges は上の定数表の値だけ（利用者の入力ではない）
        sqlx::query(crate::sql!(
            "GRANT {privileges} ON {schema}.{table} TO {app_role}"
        ))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    apply_audit_purge(migration, app_role).await?;
    Ok(())
}

/// 監査ログの保存期間の下限（日）。
pub const MIN_AUDIT_RETENTION_DAYS: i32 = 30;

/// 監査ログの削除の専用のロール（ログインできない）の名前。普段用のロールの名前から作る（インスタンスごとに別）。
pub fn audit_purger_role(app_role: &Ident) -> Result<Ident> {
    Ok(Ident::new(&format!("{}_audit_purger", app_role.as_str()))?)
}

/// 保存期間を過ぎた監査ログの削除（11-permissions-and-audit.md「保護と保存期間」、Q111、計画の14番）。
///
/// - 削除できるのは、ログインできない専用のロールだけ。そのロールが持つ関数 `cms.purge_audit_log(日数)` を、
///   普段用のロールは呼べる（行を直接消す権限は持たない）。関数の中身は固定で、保存期間より古い行しか消さず、
///   30日より短い指定は受け付けない。
/// - 書き出し先に送り終えていない記録は消さない（停止した書き出し先は除く）。インスタンス全体の書き出し先は
///   すべての記録を、ワークスペースの書き出し先はそのワークスペースの記録を待つ。
/// - 何度実行しても同じ結果になる（起動のたびに作り直す）。
async fn apply_audit_purge(migration: &PgPool, app_role: &Ident) -> Result<()> {
    let schema = Ident::new(SHARED_SCHEMA)?;
    let purger = audit_purger_role(app_role)?;
    let me = crate::db::current_role(migration).await?;
    let mut tx = migration.begin().await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = $1)")
            .bind(purger.as_str())
            .fetch_one(&mut *tx)
            .await?;
    if !exists {
        sqlx::query(crate::sql!("CREATE ROLE {purger} NOLOGIN NOINHERIT"))
            .execute(&mut *tx)
            .await?;
    }
    // 関数の持ち主を変えるため、移行のロールが一時的にこのロールになれるようにする
    sqlx::query(crate::sql!(
        "GRANT {purger} TO {me} WITH INHERIT TRUE, SET TRUE"
    ))
    .execute(&mut *tx)
    .await?;
    sqlx::query(crate::sql!(
        "GRANT USAGE, CREATE ON SCHEMA {schema} TO {purger}"
    ))
    .execute(&mut *tx)
    .await?;
    sqlx::query(crate::sql!(
        "REVOKE ALL ON {schema}.audit_log FROM {purger}"
    ))
    .execute(&mut *tx)
    .await?;
    sqlx::query(crate::sql!(
        "GRANT SELECT, DELETE ON {schema}.audit_log TO {purger}"
    ))
    .execute(&mut *tx)
    .await?;
    sqlx::query(crate::sql!(
        "GRANT SELECT ON {schema}.audit_export_cursors, {schema}.audit_sinks TO {purger}"
    ))
    .execute(&mut *tx)
    .await?;
    // 中身は固定（引数は日数だけ）。search_path を固定し、SECURITY DEFINER で持ち主（専用のロール）として動く
    sqlx::query(crate::sql!(
        r#"CREATE OR REPLACE FUNCTION {schema}.purge_audit_log(retention_days integer) RETURNS bigint
           LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, pg_temp AS $fn$
           DECLARE
             boundary timestamptz;
             instance_cursor timestamptz;
             deleted bigint;
           BEGIN
             IF retention_days IS NULL OR retention_days < {MIN_AUDIT_RETENTION_DAYS} THEN
               RAISE EXCEPTION 'retention_days must be at least {MIN_AUDIT_RETENTION_DAYS}';
             END IF;
             boundary := now() - make_interval(days => retention_days);
             SELECT min(cursor_at) INTO instance_cursor FROM {schema}.audit_export_cursors WHERE stopped_at IS NULL;
             DELETE FROM {schema}.audit_log a
             WHERE a.created_at < boundary
               AND (instance_cursor IS NULL OR a.created_at < instance_cursor)
               AND NOT EXISTS (
                 SELECT 1 FROM {schema}.audit_sinks s
                 WHERE s.workspace_id = a.workspace_id AND s.stopped_at IS NULL AND s.cursor_at <= a.created_at);
             GET DIAGNOSTICS deleted = ROW_COUNT;
             RETURN deleted;
           END
           $fn$"#
    ))
    .execute(&mut *tx)
    .await?;
    sqlx::query(crate::sql!(
        "ALTER FUNCTION {schema}.purge_audit_log(integer) OWNER TO {purger}"
    ))
    .execute(&mut *tx)
    .await?;
    sqlx::query(crate::sql!(
        "REVOKE ALL ON FUNCTION {schema}.purge_audit_log(integer) FROM PUBLIC"
    ))
    .execute(&mut *tx)
    .await?;
    sqlx::query(crate::sql!(
        "GRANT EXECUTE ON FUNCTION {schema}.purge_audit_log(integer) TO {app_role}"
    ))
    .execute(&mut *tx)
    .await?;
    // 持ち主を変えた後は、スキーマに物を作る権限と、移行のロールのなりすましを外す
    sqlx::query(crate::sql!(
        "REVOKE CREATE ON SCHEMA {schema} FROM {purger}"
    ))
    .execute(&mut *tx)
    .await?;
    sqlx::query(crate::sql!("REVOKE {purger} FROM {me}"))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_ordered_and_unique() {
        let versions: Vec<i32> = MIGRATIONS.iter().map(|m| m.0).collect();
        let mut sorted = versions.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(versions, sorted);
    }

    #[test]
    fn privilege_table_uses_valid_identifiers_and_known_privileges() {
        for (table, privileges) in TABLE_PRIVILEGES {
            assert!(Ident::new(table).is_ok(), "{table}");
            for p in privileges.split(',').map(str::trim) {
                assert!(
                    ["SELECT", "INSERT", "UPDATE", "DELETE"].contains(&p),
                    "{table}: {p}"
                );
            }
        }
        let audit = TABLE_PRIVILEGES
            .iter()
            .find(|(t, _)| *t == "audit_log")
            .unwrap();
        assert!(
            !audit.1.contains("UPDATE") && !audit.1.contains("DELETE"),
            "audit_log must be append-only"
        );
    }
}
