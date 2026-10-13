//! テンプレートのデプロイ（Q52, Q53, Q102）。
//!
//! 1. マニフェストを検査する（CLIの検査はサーバーでもやり直す）
//! 2. 破壊的な変更は、明示的な指定がなければ拒否する（INV-SCH-02）
//! 3. 新しい版として保存し、そのテンプレートを使うワークスペースを1つずつ別のトランザクションで移行する。
//!    失敗したワークスペースは「移行待ち」にし、ほかのワークスペースには影響させない（INV-SCH-05）。
//!    移行待ちのワークスペースは、次のデプロイのときに最新の版への移行をやり直す。
//!
//! デプロイとワークスペースの作成だけが、移行・ロールの作成用の接続を使う（Q173, Q179）。

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use super::diff::{Breaking, breaking_changes};
use super::manifest::{Manifest, validate};
use super::migrate;
use crate::audit;
use crate::workspace_db::WorkspaceDb;

/// デプロイを同時に1つだけにするための、アドバイザリーロックのキー（任意の固定値）。
const DEPLOY_LOCK_KEY: i64 = 0x636d_735f_6465_706c; // "cms_depl"

/// デプロイを受け付けなかった理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected {
    /// マニフェストが正しくない（場所と理由の一覧）。
    Invalid(Vec<String>),
    /// 明示的な指定のない、破壊的な変更。
    Destructive(Vec<Breaking>),
    /// 明示的な指定のない、登録済みのクエリを壊す変更（Q54）。
    BreaksQueries(Vec<BrokenQuery>),
}

/// 新しいスキーマで壊れる登録済みのクエリ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrokenQuery {
    pub workspace_id: Uuid,
    pub query_id: String,
    pub problems: Vec<String>,
}

/// デプロイのときの、登録済みのクエリの検証の設定。
#[derive(Debug, Clone, Copy)]
pub struct QueryCheck {
    /// 壊れるクエリがあってもデプロイする（明示的な指定）。
    pub allow_broken: bool,
    pub limits: crate::delivery::schema::QueryLimits,
    /// この日数より長く使われていないクエリは検証しない（Q151）。
    pub unused_days: u32,
}

/// ワークスペースごとの結果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceResult {
    pub workspace_id: Uuid,
    /// ok: 最新の版まで移行した、pending: 移行待ち
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub template: String,
    pub version: i32,
    /// 最新の版と同じ内容だった（新しい版は作らず、移行待ちのワークスペースだけやり直した）。
    pub unchanged: bool,
    pub workspaces: Vec<WorkspaceResult>,
}

/// テンプレートの版のマニフェスト。
pub async fn load_version<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    template: &str,
    version: i32,
) -> Result<Option<Manifest>> {
    let raw: Option<String> = sqlx::query_scalar(
        "SELECT manifest::text FROM cms.template_versions WHERE template = $1 AND version = $2",
    )
    .bind(template)
    .bind(version)
    .fetch_optional(executor)
    .await?;
    raw.map(|r| serde_json::from_str(&r).context("stored manifest cannot be read"))
        .transpose()
}

async fn latest_version(
    conn: &mut PgConnection,
    template: &str,
) -> Result<Option<(i32, Manifest)>> {
    let row: Option<(i32, String)> = sqlx::query_as(
        "SELECT version, manifest::text FROM cms.template_versions
         WHERE template = $1 ORDER BY version DESC LIMIT 1",
    )
    .bind(template)
    .fetch_optional(&mut *conn)
    .await?;
    row.map(|(v, m)| Ok((v, serde_json::from_str(&m)?)))
        .transpose()
}

/// テンプレートをデプロイする。`app` は監査ログの記録に使う。
pub async fn deploy(
    migration: &PgPool,
    app: &PgPool,
    template: &str,
    manifest: Manifest,
    allow_destructive: bool,
    queries: QueryCheck,
    actor: Uuid,
) -> Result<Result<Outcome, Rejected>> {
    let problems = validate(&manifest);
    if !problems.is_empty() {
        return Ok(Err(Rejected::Invalid(problems)));
    }
    let mut conn = migration.acquire().await?;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(DEPLOY_LOCK_KEY)
        .execute(&mut *conn)
        .await?;
    let result = deploy_locked(
        &mut conn,
        app,
        template,
        manifest,
        allow_destructive,
        queries,
        actor,
    )
    .await;
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(DEPLOY_LOCK_KEY)
        .execute(&mut *conn)
        .await?;
    result
}

async fn deploy_locked(
    conn: &mut PgConnection,
    app: &PgPool,
    template: &str,
    manifest: Manifest,
    allow_destructive: bool,
    queries: QueryCheck,
    actor: Uuid,
) -> Result<Result<Outcome, Rejected>> {
    let latest = latest_version(conn, template).await?;
    let breaking = breaking_changes(latest.as_ref().map(|(_, m)| m), &manifest);
    if !breaking.is_empty() && !allow_destructive {
        return Ok(Err(Rejected::Destructive(breaking)));
    }
    // 登録済みのクエリを、新しいスキーマで検証する（Q54、INV-SCH-03）。破壊的な変更を許しても、この検証は別に行う
    if !queries.allow_broken && latest.as_ref().is_none_or(|(_, m)| *m != manifest) {
        let broken = broken_queries(conn, template, &manifest, queries).await?;
        if !broken.is_empty() {
            return Ok(Err(Rejected::BreaksQueries(broken)));
        }
    }
    let (version, unchanged) = match &latest {
        Some((v, m)) if *m == manifest => (*v, true),
        _ => {
            let version = latest.as_ref().map_or(1, |(v, _)| v + 1);
            sqlx::query(
                "INSERT INTO cms.template_versions (template, version, manifest, deployed_by)
                 VALUES ($1, $2, $3::jsonb, $4)",
            )
            .bind(template)
            .bind(version)
            .bind(serde_json::to_string(&manifest)?)
            .bind(actor)
            .execute(&mut *conn)
            .await?;
            (version, false)
        }
    };
    audit::record(
        app,
        None,
        Some(actor),
        audit::action::TEMPLATE_DEPLOYED,
        &[],
    )
    .await?;

    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM cms.workspaces
         WHERE template = $1 AND (schema_version IS DISTINCT FROM $2 OR schema_state <> 'ok')
         ORDER BY created_at, id",
    )
    .bind(template)
    .bind(version)
    .fetch_all(&mut *conn)
    .await?;
    let mut workspaces = Vec::with_capacity(ids.len());
    for id in ids {
        workspaces.push(
            migrate_workspace(conn, app, id, template, version, &manifest, Some(actor)).await?,
        );
    }
    Ok(Ok(Outcome {
        template: template.to_owned(),
        version,
        unchanged,
        workspaces,
    }))
}

/// テンプレートを使うワークスペースの登録済みのクエリのうち、新しいスキーマで検証に通らないもの。
/// 移行・ロールの作成用の接続で、各ワークスペースのスキーマのテーブルを読む。
async fn broken_queries(
    conn: &mut PgConnection,
    template: &str,
    manifest: &Manifest,
    check: QueryCheck,
) -> Result<Vec<BrokenQuery>> {
    let schema = crate::delivery::schema::build(manifest, false, check.limits)?;
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM cms.workspaces WHERE template = $1 ORDER BY created_at, id",
    )
    .bind(template)
    .fetch_all(&mut *conn)
    .await?;
    let mut broken = Vec::new();
    for workspace_id in ids {
        let db = WorkspaceDb::names_for(workspace_id);
        let schema_name = &db.schema;
        let queries: Vec<(String, String)> = sqlx::query_as(crate::sql!(
            "SELECT id, query FROM {schema_name}.queries
             WHERE last_used_at > now() - make_interval(days => $1) ORDER BY id"
        ))
        .bind(i32::try_from(check.unused_days).unwrap_or(i32::MAX))
        .fetch_all(&mut *conn)
        .await?;
        for (query_id, query) in queries {
            if let Err(problems) = crate::delivery::validate::check(&schema, &query).await {
                broken.push(BrokenQuery {
                    workspace_id,
                    query_id,
                    problems,
                });
            }
        }
    }
    Ok(broken)
}

/// 1つのワークスペースを、テンプレートの指定の版へ移行する（別のトランザクション）。
async fn migrate_workspace(
    conn: &mut PgConnection,
    app: &PgPool,
    workspace_id: Uuid,
    template: &str,
    version: i32,
    manifest: &Manifest,
    actor: Option<Uuid>,
) -> Result<WorkspaceResult> {
    let db = WorkspaceDb::names_for(workspace_id);
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    let applied: Option<i32> =
        sqlx::query_scalar("SELECT schema_version FROM cms.workspaces WHERE id = $1 FOR UPDATE")
            .bind(workspace_id)
            .fetch_one(&mut *tx)
            .await?;
    let old = match applied {
        Some(v) => Some(
            load_version(&mut *tx, template, v)
                .await?
                .context("applied template version is missing")?,
        ),
        None => None,
    };
    let result = migrate::apply(&mut tx, &db, old.as_ref(), manifest).await?;
    match result {
        Ok(()) => {
            sqlx::query(
                "UPDATE cms.workspaces SET schema_version = $2, schema_state = 'ok',
                        schema_target_version = NULL, schema_error = NULL
                 WHERE id = $1",
            )
            .bind(workspace_id)
            .bind(version)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            audit::record(
                app,
                Some(workspace_id),
                actor,
                audit::action::SCHEMA_MIGRATED,
                &[],
            )
            .await?;
            Ok(WorkspaceResult {
                workspace_id,
                state: "ok",
                error: None,
            })
        }
        Err(failure) => {
            tx.rollback().await?;
            let code = failure.code();
            sqlx::query(
                "UPDATE cms.workspaces SET schema_state = 'pending', schema_target_version = $2, schema_error = $3
                 WHERE id = $1",
            )
            .bind(workspace_id)
            .bind(version)
            .bind(&code)
            .execute(&mut *conn)
            .await?;
            audit::record(
                app,
                Some(workspace_id),
                actor,
                audit::action::SCHEMA_MIGRATION_FAILED,
                &[],
            )
            .await?;
            Ok(WorkspaceResult {
                workspace_id,
                state: "pending",
                error: Some(code),
            })
        }
    }
}

/// 作ったばかりのワークスペースに、テンプレートの最新の版を適用する（まだデプロイされていなければ何もしない）。
pub async fn apply_latest_to_new_workspace(
    migration: &PgPool,
    app: &PgPool,
    workspace_id: Uuid,
    template: &str,
    actor: Uuid,
) -> Result<()> {
    let mut conn = migration.acquire().await?;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(DEPLOY_LOCK_KEY)
        .execute(&mut *conn)
        .await?;
    let result = async {
        if let Some((version, manifest)) = latest_version(&mut conn, template).await? {
            migrate_workspace(
                &mut conn,
                app,
                workspace_id,
                template,
                version,
                &manifest,
                Some(actor),
            )
            .await?;
        }
        anyhow::Ok(())
    }
    .await;
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(DEPLOY_LOCK_KEY)
        .execute(&mut *conn)
        .await?;
    result
}
