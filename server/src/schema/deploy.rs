//! テンプレートのデプロイ（Q52, Q53, Q102）。
//!
//! 1. マニフェストを検査する（CLIの検査はサーバーでもやり直す）
//! 2. 破壊的な変更は、明示的な指定がなければ拒否する（INV-SCH-02）
//! 3. 新しい版として保存し、そのテンプレートを使うワークスペースを1つずつ別のトランザクションで移行する。
//!    失敗したワークスペースは「移行待ち」にし、ほかのワークスペースには影響させない（INV-SCH-05）。
//!    移行待ちのワークスペースは、次のデプロイのときに最新の版への移行をやり直す。
//!
//! デプロイとワークスペースの作成だけが、移行・ロールの作成用の接続を使う（Q173, Q179）。
//!
//! `check` は、デプロイと同じ検査（1・2と登録済みのクエリの検証）を、何も書き込まずに行う（CLIがデプロイの前に結果を見せるため）。
//! 判定のコードはデプロイと同じものを使う。

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use super::diff::{Breaking, breaking_changes};
use super::manifest::{self, Manifest};
use super::migrate;
use crate::audit;
use crate::workspace_db::WorkspaceDb;

/// デプロイを同時に1つだけにするための、アドバイザリーロックのキー（任意の固定値）。
const DEPLOY_LOCK_KEY: i64 = 0x636d_735f_6465_706c; // "yomi_depl"

/// デプロイを受け付けなかった理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected {
    /// マニフェストが正しくない（場所と理由の一覧）。
    Invalid(Vec<String>),
    /// 明示的な指定のない、破壊的な変更。
    Destructive(Vec<Breaking>),
}

/// 壊れるクエリがあるため移行しなかったワークスペースの、理由のコード。
pub const BREAKS_QUERIES: &str = "breaks_registered_queries";

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
    /// 新しい版で壊れるため、移行しなかった理由の登録済みのクエリ（明示的な指定がないとき。INV-SCH-03）。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub broken_queries: Vec<BrokenQuery>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub template: String,
    pub version: i32,
    /// 最新の版と同じ内容だった（新しい版は作らず、移行待ちのワークスペースだけやり直した）。
    pub unchanged: bool,
    pub workspaces: Vec<WorkspaceResult>,
    /// 公開URLの決め方（テンプレートか関数）が変わったコンテンツタイプ（URLの再計算に使う。応答には含めない）。
    #[serde(skip)]
    pub url_rule_changes: Vec<String>,
}

/// テンプレートの版のマニフェスト。
pub async fn load_version<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    template: &str,
    version: i32,
) -> Result<Option<Manifest>> {
    let raw: Option<String> = sqlx::query_scalar(
        "SELECT manifest::text FROM yomi.template_versions WHERE template = $1 AND version = $2",
    )
    .bind(template)
    .bind(version)
    .fetch_optional(executor)
    .await?;
    raw.map(|r| serde_json::from_str(&r).context("stored manifest cannot be read"))
        .transpose()
}

/// テンプレートの最新の版（同梱のテンプレートの一覧で、デプロイ済みの版と比べるため）。
pub async fn load_latest(
    conn: &mut PgConnection,
    template: &str,
) -> Result<Option<(i32, Manifest)>> {
    latest_version(conn, template).await
}

async fn latest_version(
    conn: &mut PgConnection,
    template: &str,
) -> Result<Option<(i32, Manifest)>> {
    let row: Option<(i32, String)> = sqlx::query_as(
        "SELECT version, manifest::text FROM yomi.template_versions
         WHERE template = $1 ORDER BY version DESC LIMIT 1",
    )
    .bind(template)
    .fetch_optional(&mut *conn)
    .await?;
    row.map(|(v, m)| Ok((v, serde_json::from_str(&m)?)))
        .transpose()
}

/// テンプレートをデプロイする。
/// デプロイの試行の結果（何も書き込まない）。明示的な指定の有無にかかわらず、見つかった問題をすべて返す。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    /// 試行であることの印（応答でデプロイの結果と見分けるため。常に true）。
    pub dry_run: bool,
    /// マニフェストの問題（あれば、ほかの検査はしない）。
    pub problems: Vec<String>,
    /// 破壊的な変更の説明。
    pub destructive: Vec<String>,
    /// 新しいスキーマで壊れる登録済みのクエリ。
    pub broken_queries: Vec<BrokenQuery>,
    /// 最新の版と同じ内容か。
    pub unchanged: bool,
    /// いまの最新の版（まだなければ null）。
    pub current_version: Option<i32>,
    /// このテンプレートを使うワークスペースの数。
    pub workspaces: usize,
}

/// マニフェストの検査に、関数を組み込みJSで読み込めるかの検査を加えたもの（CLIを通さないデプロイでも行う。Q102）。
pub async fn validate(engine: &crate::script::Engine, m: &Manifest) -> Vec<String> {
    let mut problems = manifest::validate(m);
    if problems.is_empty() {
        for (path, script) in manifest::scripts(m) {
            if let Err(f) = engine.check(script).await {
                problems.push(format!(
                    "{path}.source: cannot be loaded ({}); it must be an ES module whose default export is a function",
                    f.describe()
                ));
            }
        }
    }
    problems
}

/// デプロイと同じ検査を、書き込まずに行う。
pub async fn check(
    engine: &crate::script::Engine,
    migration: &PgPool,
    template: &str,
    manifest: &Manifest,
    queries: QueryCheck,
) -> Result<Check> {
    let mut out = Check {
        dry_run: true,
        problems: validate(engine, manifest).await,
        destructive: Vec::new(),
        broken_queries: Vec::new(),
        unchanged: false,
        current_version: None,
        workspaces: 0,
    };
    let mut conn = migration.acquire().await?;
    let latest = latest_version(&mut conn, template).await?;
    out.current_version = latest.as_ref().map(|(v, _)| *v);
    out.workspaces = usize::try_from(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM yomi.workspaces WHERE template = $1")
            .bind(template)
            .fetch_one(&mut *conn)
            .await?,
    )
    .unwrap_or_default();
    if !out.problems.is_empty() {
        return Ok(out);
    }
    out.unchanged = latest.as_ref().is_some_and(|(_, m)| m == manifest);
    out.destructive = breaking_changes(latest.as_ref().map(|(_, m)| m), manifest)
        .iter()
        .map(Breaking::describe)
        .collect();
    if !out.unchanged {
        out.broken_queries = broken_queries(&mut conn, template, manifest, queries).await?;
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
pub async fn deploy(
    engine: &crate::script::Engine,
    migration: &PgPool,
    template: &str,
    manifest: Manifest,
    allow_destructive: bool,
    queries: QueryCheck,
    actor: Uuid,
) -> Result<Result<Outcome, Rejected>> {
    let problems = validate(engine, &manifest).await;
    if !problems.is_empty() {
        return Ok(Err(Rejected::Invalid(problems)));
    }
    let template = template.to_owned();
    with_deploy_lock(migration, move |conn| {
        Box::pin(async move {
            deploy_locked(conn, &template, manifest, allow_destructive, queries, actor).await
        })
    })
    .await
}

/// デプロイのロックを取って `work` を行い、ロックを外す。
///
/// ロックは接続ごとのもの（アドバイザリーロック）なので、取ってから外すまでを別のタスクで動かす。リクエストの途中で
/// 相手が接続を切っても、そのタスクは最後まで走ってロックを外す（呼んだ側の処理が打ち切られて、ロックを持ったままの
/// 接続がプールに戻ることを防ぐ。侵入テストの前の確認で見つけた）。
async fn with_deploy_lock<T: Send + 'static>(
    migration: &PgPool,
    work: impl for<'c> FnOnce(
        &'c mut PgConnection,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<T>> + Send + 'c>,
    > + Send
    + 'static,
) -> Result<T> {
    let pool = migration.clone();
    tokio::spawn(async move {
        let mut conn = pool.acquire().await?;
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(DEPLOY_LOCK_KEY)
            .execute(&mut *conn)
            .await?;
        let result = work(&mut conn).await;
        let unlocked = sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(DEPLOY_LOCK_KEY)
            .execute(&mut *conn)
            .await;
        if unlocked.is_err() {
            // 外せなかった接続はプールに戻さず閉じる（接続が閉じればロックも外れる）
            let _ = conn.detach();
        }
        unlocked?;
        result
    })
    .await?
}

async fn deploy_locked(
    conn: &mut PgConnection,
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
    // 登録済みのクエリの検証（Q54、INV-SCH-03）は、ワークスペースごとに移行の前に行う。壊れるクエリを持つワークスペースだけを
    // 移行待ちに残し、ほかのワークスペースは移行する（1つのワークスペースのクエリで、全体のデプロイを止めさせない。R42）
    let schema = if queries.allow_broken {
        None
    } else {
        Some(crate::delivery::schema::build(
            &manifest,
            false,
            queries.limits,
        )?)
    };
    // 版の記録と監査ログは同じトランザクションで書く（R34）
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    let (version, unchanged) = match &latest {
        Some((v, m)) if *m == manifest => (*v, true),
        _ => {
            let version = latest.as_ref().map_or(1, |(v, _)| v + 1);
            sqlx::query(
                "INSERT INTO yomi.template_versions (template, version, manifest, deployed_by)
                 VALUES ($1, $2, $3::jsonb, $4)",
            )
            .bind(template)
            .bind(version)
            .bind(serde_json::to_string(&manifest)?)
            .bind(actor)
            .execute(&mut *tx)
            .await?;
            (version, false)
        }
    };
    audit::record(
        &mut *tx,
        None,
        Some(actor),
        audit::action::TEMPLATE_DEPLOYED,
        &[],
    )
    .await?;
    tx.commit().await?;

    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM yomi.workspaces
         WHERE template = $1 AND (schema_version IS DISTINCT FROM $2 OR schema_state <> 'ok')
         ORDER BY created_at, id",
    )
    .bind(template)
    .bind(version)
    .fetch_all(&mut *conn)
    .await?;
    let mut workspaces = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(schema) = &schema {
            let broken = broken_in_workspace(conn, schema, id, queries.unused_days).await?;
            if !broken.is_empty() {
                workspaces.push(hold_for_queries(conn, id, version, Some(actor), broken).await?);
                continue;
            }
        }
        workspaces
            .push(migrate_workspace(conn, id, template, version, &manifest, Some(actor)).await?);
    }
    Ok(Ok(Outcome {
        template: template.to_owned(),
        version,
        unchanged,
        workspaces,
        url_rule_changes: crate::content::urls::url_rule_changes(
            latest.as_ref().map(|(_, m)| m),
            &manifest,
        ),
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
        "SELECT id FROM yomi.workspaces WHERE template = $1 ORDER BY created_at, id",
    )
    .bind(template)
    .fetch_all(&mut *conn)
    .await?;
    let mut broken = Vec::new();
    for workspace_id in ids {
        broken.extend(broken_in_workspace(conn, &schema, workspace_id, check.unused_days).await?);
    }
    Ok(broken)
}

/// 1つのワークスペースの登録済みのクエリのうち、新しいスキーマで検証に通らないもの。
async fn broken_in_workspace(
    conn: &mut PgConnection,
    schema: &async_graphql::dynamic::Schema,
    workspace_id: Uuid,
    unused_days: u32,
) -> Result<Vec<BrokenQuery>> {
    let db = WorkspaceDb::names_for(workspace_id);
    let schema_name = &db.schema;
    let queries: Vec<(String, String)> = sqlx::query_as(crate::sql!(
        "SELECT id, query FROM {schema_name}.queries
         WHERE last_used_at > now() - make_interval(days => $1) ORDER BY id"
    ))
    .bind(i32::try_from(unused_days).unwrap_or(i32::MAX))
    .fetch_all(&mut *conn)
    .await?;
    let mut broken = Vec::new();
    for (query_id, query) in queries {
        if let Err(problems) = crate::delivery::validate::check(schema, &query).await {
            broken.push(BrokenQuery {
                workspace_id,
                query_id,
                problems,
            });
        }
    }
    Ok(broken)
}

/// 壊れるクエリがあるワークスペースを、旧版のまま移行待ちにする（配信は続き、編集だけを断る。INV-SCH-04）。
/// 次のデプロイ（同じ内容でもよい）で、もう一度確かめる。
async fn hold_for_queries(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    version: i32,
    actor: Option<Uuid>,
    broken: Vec<BrokenQuery>,
) -> Result<WorkspaceResult> {
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    sqlx::query(
        "UPDATE yomi.workspaces SET schema_state = 'pending', schema_target_version = $2, schema_error = $3
         WHERE id = $1",
    )
    .bind(workspace_id)
    .bind(version)
    .bind(BREAKS_QUERIES)
    .execute(&mut *tx)
    .await?;
    audit::record(
        &mut *tx,
        Some(workspace_id),
        actor,
        audit::action::SCHEMA_MIGRATION_FAILED,
        &[],
    )
    .await?;
    tx.commit().await?;
    Ok(WorkspaceResult {
        workspace_id,
        state: "pending",
        error: Some(BREAKS_QUERIES.to_owned()),
        broken_queries: broken,
    })
}

/// 1つのワークスペースを、テンプレートの指定の版へ移行する（別のトランザクション）。
async fn migrate_workspace(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    template: &str,
    version: i32,
    manifest: &Manifest,
    actor: Option<Uuid>,
) -> Result<WorkspaceResult> {
    let db = WorkspaceDb::names_for(workspace_id);
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    let applied: Option<i32> =
        sqlx::query_scalar("SELECT schema_version FROM yomi.workspaces WHERE id = $1 FOR UPDATE")
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
                "UPDATE yomi.workspaces SET schema_version = $2, schema_state = 'ok',
                        schema_target_version = NULL, schema_error = NULL
                 WHERE id = $1",
            )
            .bind(workspace_id)
            .bind(version)
            .execute(&mut *tx)
            .await?;
            audit::record(
                &mut *tx,
                Some(workspace_id),
                actor,
                audit::action::SCHEMA_MIGRATED,
                &[],
            )
            .await?;
            tx.commit().await?;
            Ok(WorkspaceResult {
                workspace_id,
                state: "ok",
                error: None,
                broken_queries: Vec::new(),
            })
        }
        Err(failure) => {
            tx.rollback().await?;
            let code = failure.code();
            let mut tx = sqlx::Connection::begin(&mut *conn).await?;
            sqlx::query(
                "UPDATE yomi.workspaces SET schema_state = 'pending', schema_target_version = $2, schema_error = $3
                 WHERE id = $1",
            )
            .bind(workspace_id)
            .bind(version)
            .bind(&code)
            .execute(&mut *tx)
            .await?;
            audit::record(
                &mut *tx,
                Some(workspace_id),
                actor,
                audit::action::SCHEMA_MIGRATION_FAILED,
                &[],
            )
            .await?;
            tx.commit().await?;
            Ok(WorkspaceResult {
                workspace_id,
                state: "pending",
                error: Some(code),
                broken_queries: Vec::new(),
            })
        }
    }
}

/// 作ったばかりのワークスペースに、テンプレートの最新の版を適用する（まだデプロイされていなければ何もしない）。
pub async fn apply_latest_to_new_workspace(
    migration: &PgPool,
    workspace_id: Uuid,
    template: &str,
    actor: Uuid,
) -> Result<()> {
    let template = template.to_owned();
    with_deploy_lock(migration, move |conn| {
        Box::pin(async move {
            if let Some((version, manifest)) = latest_version(conn, &template).await? {
                migrate_workspace(
                    conn,
                    workspace_id,
                    &template,
                    version,
                    &manifest,
                    Some(actor),
                )
                .await?;
            }
            Ok(())
        })
    })
    .await
}
