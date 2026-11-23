//! テンプレートのデプロイ（Q52, Q53, Q102）。
//!
//! 1. マニフェストを検査する（CLIの検査はサーバーでもやり直す）
//! 2. 破壊的な変更は、明示的な指定がなければ拒否する（INV-SCH-02）
//! 3. 登録済みのクエリが1つでも壊れるなら、明示的な指定がなければ全体を拒否する（INV-SCH-03）
//! 4. テンプレートを使うすべてのワークスペースを、1つのトランザクションで移行する。1つでも失敗すれば、全体を巻き戻して
//!    拒否する（INV-SCH-04・07）。テンプレートの版は持たず、最新のマニフェストだけを持つ（2026-11-23、持ち主の判断）。
//!
//! デプロイとワークスペースの作成だけが、移行・ロールの作成用の接続を使う（Q173, Q179）。
//!
//! `check` は、デプロイと同じ検査（1〜3）を、何も書き込まずに行う（CLIがデプロイの前に結果を見せるため）。
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

/// デプロイを受け付けなかった理由。どの場合も、どのワークスペースにも適用していない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected {
    /// マニフェストが正しくない（場所と理由の一覧）。
    Invalid(Vec<String>),
    /// 明示的な指定のない、破壊的な変更。
    Destructive(Vec<Breaking>),
    /// 明示的な指定がなく、新しいスキーマで壊れる登録済みのクエリがある（INV-SCH-03）。
    BrokenQueries(Vec<BrokenQuery>),
    /// あるワークスペースの移行が失敗した（INV-SCH-04）。`code` は機械的に判別できる理由で、記事の内容は含めない。
    MigrationFailed { workspace_id: Uuid, code: String },
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub template: String,
    /// 最後に内容を変えてデプロイした日時。
    pub deployed_at: String,
    /// デプロイ済みのものと同じ内容だった（まだ適用していないワークスペースにだけ適用した）。
    pub unchanged: bool,
    /// 適用したワークスペース。
    pub workspaces: Vec<Uuid>,
    /// 公開URLの決め方（テンプレートか関数）が変わったコンテンツタイプ（URLの再計算に使う。応答には含めない）。
    #[serde(skip)]
    pub url_rule_changes: Vec<String>,
}

/// デプロイ済みのテンプレート（最新のマニフェストと、デプロイした日時）。
#[derive(Debug, Clone)]
pub struct Deployed {
    pub manifest: Manifest,
    pub deployed_at: String,
}

/// デプロイ済みのテンプレート（まだデプロイしていなければ None）。
pub async fn load<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    template: &str,
) -> Result<Option<Deployed>> {
    // デプロイした日時は、マイクロ秒までの UTC の文字列にする（利用者に見せ、配信のスキーマのキャッシュのキーにも使う）
    let row: Option<(String, String)> = sqlx::query_as(
        r#"SELECT manifest::text, to_char(deployed_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
           FROM yomi.templates WHERE name = $1"#,
    )
    .bind(template)
    .fetch_optional(executor)
    .await?;
    row.map(|(m, deployed_at)| {
        Ok(Deployed {
            manifest: serde_json::from_str(&m).context("stored manifest cannot be read")?,
            deployed_at,
        })
    })
    .transpose()
}

/// デプロイ済みのテンプレートの名前と、デプロイした日時（名前の順）。
pub async fn list<'e>(executor: impl sqlx::PgExecutor<'e>) -> Result<Vec<(String, String)>> {
    Ok(sqlx::query_as(
        r#"SELECT name, to_char(deployed_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
           FROM yomi.templates ORDER BY name"#,
    )
    .fetch_all(executor)
    .await?)
}

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
    /// デプロイ済みのものと同じ内容か。
    pub unchanged: bool,
    /// 最後にデプロイした日時（まだなければ null）。
    pub deployed_at: Option<String>,
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
        deployed_at: None,
        workspaces: 0,
    };
    let mut conn = migration.acquire().await?;
    let deployed = load(&mut *conn, template).await?;
    out.deployed_at = deployed.as_ref().map(|d| d.deployed_at.clone());
    let ids = workspaces_on(&mut conn, template, false).await?;
    out.workspaces = ids.len();
    if !out.problems.is_empty() {
        return Ok(out);
    }
    out.unchanged = deployed.as_ref().is_some_and(|d| d.manifest == *manifest);
    out.destructive = breaking_changes(deployed.as_ref().map(|d| &d.manifest), manifest)
        .iter()
        .map(Breaking::describe)
        .collect();
    if !out.unchanged {
        out.broken_queries = broken_queries(&mut conn, &ids, manifest, queries).await?;
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
pub(crate) async fn with_deploy_lock<T: Send + 'static>(
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
    let deployed = load(&mut *conn, template).await?;
    let old = deployed.as_ref().map(|d| &d.manifest);
    let breaking = breaking_changes(old, &manifest);
    if !breaking.is_empty() && !allow_destructive {
        return Ok(Err(Rejected::Destructive(breaking)));
    }
    let unchanged = old.is_some_and(|m| *m == manifest);
    // 同じ内容なら、まだ適用していないワークスペースにだけ適用する
    let targets = workspaces_on(conn, template, unchanged).await?;
    if !queries.allow_broken && !unchanged {
        let broken = broken_queries(conn, &targets, &manifest, queries).await?;
        if !broken.is_empty() {
            return Ok(Err(Rejected::BrokenQueries(broken)));
        }
    }

    // マニフェストの記録、すべてのワークスペースの移行、監査ログを、1つのトランザクションで行う（全部か、何もしないか）
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    if !unchanged {
        sqlx::query(
            "INSERT INTO yomi.templates (name, manifest, deployed_by, deployed_at)
             VALUES ($1, $2::jsonb, $3, now())
             ON CONFLICT (name) DO UPDATE
             SET manifest = EXCLUDED.manifest, deployed_by = EXCLUDED.deployed_by, deployed_at = EXCLUDED.deployed_at",
        )
        .bind(template)
        .bind(serde_json::to_string(&manifest)?)
        .bind(actor)
        .execute(&mut *tx)
        .await?;
    }
    for &workspace_id in &targets {
        match migrate_workspace(&mut tx, workspace_id, old, &manifest).await {
            Ok(Ok(())) => {}
            Ok(Err(failure)) => {
                tx.rollback().await?;
                record_failure(conn, workspace_id, actor).await?;
                return Ok(Err(Rejected::MigrationFailed {
                    workspace_id,
                    code: failure.code(),
                }));
            }
            Err(error) => {
                // DB のエラーで止まったときも、失敗を残す。巻き戻せなければ接続が使えないので、記録しない
                if tx.rollback().await.is_ok() {
                    record_failure(conn, workspace_id, actor).await?;
                }
                return Err(error);
            }
        }
        audit::record(
            &mut *tx,
            Some(workspace_id),
            Some(actor),
            audit::action::SCHEMA_MIGRATED,
            &[],
        )
        .await?;
    }
    audit::record(
        &mut *tx,
        None,
        Some(actor),
        audit::action::TEMPLATE_DEPLOYED,
        &[],
    )
    .await?;
    tx.commit().await?;

    let deployed_at = load(&mut *conn, template)
        .await?
        .context("the template just deployed is missing")?
        .deployed_at;
    Ok(Ok(Outcome {
        template: template.to_owned(),
        deployed_at,
        unchanged,
        workspaces: targets,
        url_rule_changes: crate::content::urls::url_rule_changes(old, &manifest),
    }))
}

/// テンプレートを使うワークスペース（`only_unapplied` なら、まだ適用していないものだけ）。作った順。
async fn workspaces_on(
    conn: &mut PgConnection,
    template: &str,
    only_unapplied: bool,
) -> Result<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM yomi.workspaces
         WHERE template = $1 AND (NOT $2 OR schema_state = 'none')
         ORDER BY created_at, id",
    )
    .bind(template)
    .bind(only_unapplied)
    .fetch_all(&mut *conn)
    .await?)
}

/// ワークスペースの登録済みのクエリのうち、新しいスキーマで検証に通らないもの。
/// 移行・ロールの作成用の接続で、各ワークスペースのスキーマのテーブルを読む。
async fn broken_queries(
    conn: &mut PgConnection,
    workspaces: &[Uuid],
    manifest: &Manifest,
    check: QueryCheck,
) -> Result<Vec<BrokenQuery>> {
    let schema = crate::delivery::schema::build(manifest, false, check.limits)?;
    let mut broken = Vec::new();
    for &workspace_id in workspaces {
        broken.extend(broken_in_workspace(conn, &schema, workspace_id, check.unused_days).await?);
    }
    Ok(broken)
}

/// 1つのワークスペースの登録済みのクエリのうち、新しいスキーマで検証に通らないもの。
pub(crate) async fn broken_in_workspace(
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

/// 1つのワークスペースを新しいマニフェストへ移行する（呼び出し側のトランザクションの中で）。
/// 適用済みのワークスペースの今のスキーマは、デプロイ済みのマニフェスト（`old`）と同じ。
async fn migrate_workspace(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    old: Option<&Manifest>,
    manifest: &Manifest,
) -> Result<Result<(), migrate::Failure>> {
    let db = WorkspaceDb::names_for(workspace_id);
    let state: String =
        sqlx::query_scalar("SELECT schema_state FROM yomi.workspaces WHERE id = $1 FOR UPDATE")
            .bind(workspace_id)
            .fetch_one(&mut *conn)
            .await?;
    let applied = if state == "ok" { old } else { None };
    if let Err(failure) = migrate::apply(conn, &db, applied, manifest).await? {
        return Ok(Err(failure));
    }
    sqlx::query("UPDATE yomi.workspaces SET schema_state = 'ok' WHERE id = $1")
        .bind(workspace_id)
        .execute(&mut *conn)
        .await?;
    Ok(Ok(()))
}

/// 移行の失敗を監査ログに残す（巻き戻したトランザクションとは別に）。
async fn record_failure(conn: &mut PgConnection, workspace_id: Uuid, actor: Uuid) -> Result<()> {
    audit::record(
        &mut *conn,
        Some(workspace_id),
        Some(actor),
        audit::action::SCHEMA_MIGRATION_FAILED,
        &[],
    )
    .await
}

/// 作ったばかりのワークスペースに、デプロイ済みのテンプレートを適用する（まだデプロイされていなければ何もしない）。
/// 失敗すれば、ワークスペースは「まだ適用していない」のまま残る。
pub async fn apply_to_new_workspace(
    migration: &PgPool,
    workspace_id: Uuid,
    template: &str,
    actor: Uuid,
) -> Result<()> {
    let template = template.to_owned();
    with_deploy_lock(migration, move |conn| {
        Box::pin(async move {
            let Some(deployed) = load(&mut *conn, &template).await? else {
                return Ok(());
            };
            let mut tx = sqlx::Connection::begin(&mut *conn).await?;
            let migrated =
                match migrate_workspace(&mut tx, workspace_id, None, &deployed.manifest).await {
                    Ok(migrated) => migrated,
                    Err(error) => {
                        // DB のエラーで止まったときも、失敗を残す（上の deploy_locked と同じ）
                        if tx.rollback().await.is_ok() {
                            record_failure(conn, workspace_id, actor).await?;
                        }
                        return Err(error);
                    }
                };
            if migrated.is_ok() {
                audit::record(
                    &mut *tx,
                    Some(workspace_id),
                    Some(actor),
                    audit::action::SCHEMA_MIGRATED,
                    &[],
                )
                .await?;
                tx.commit().await?;
            } else {
                tx.rollback().await?;
                record_failure(conn, workspace_id, actor).await?;
            }
            Ok(())
        })
    })
    .await
}
