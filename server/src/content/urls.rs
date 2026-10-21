//! 公開URLの依存の記録と再計算（05-urls-and-redirects.md「計算と保存」、Q39）。
//!
//! - URL生成関数がホスト関数で読もうとした記事を、依存先として記録する（公開のときと、再計算のとき）。
//! - 次のときに、URLを再計算するジョブを積む（外部への送信は含まないが、時間がかかりうるのでジョブで行う）:
//!   - 依存先の記事が公開・更新・訂正・非公開・削除されたとき、または依存先のURLが変わったとき
//!   - デプロイで、コンテンツタイプのURLテンプレートかURL生成関数が変わったとき（そのタイプの公開中の記事すべて）
//! - 再計算は公開中の版のフィールドで行う。URLが変わったら、記事のいまのURLを書き換え、変更履歴に記録し（版のIDは空）、
//!   URLの変更のWebhookを送る。版に記録した「公開したときのURL」は書き換えない。
//! - 関数が失敗したとき、別の公開中の記事とURLがぶつかるときは、いまのURLのまま残し、ジョブをやり直す。
//! - URLが変わった記事の依存元も再計算する。関数が呼ぶたびに違うURLを返すと止まらないので、たどる深さに上限を設ける。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use super::applied_schema;
use super::publishing::{self, Scripts};
use crate::jobs::Outcome;
use crate::keys::KeyRing;
use crate::schema::manifest::Manifest;
use crate::script::{Engine, Host};
use crate::webhooks::{self, EntryEvent};
use crate::workspace_db;

/// URLの再計算のジョブの種類。
pub const JOB_KIND: &str = "url";
const MAX_ATTEMPTS: i32 = 5;
/// URLが変わった記事の依存元を、たどる深さの上限。
const MAX_DEPTH: u32 = 5;

/// 記事の依存先を記録し直す。ワークスペースのロールのトランザクションの中で呼ぶ。
pub async fn record_dependencies(
    conn: &mut PgConnection,
    entry_id: Uuid,
    depends_on: &[Uuid],
) -> Result<()> {
    sqlx::query("DELETE FROM url_dependencies WHERE entry_id = $1")
        .bind(entry_id)
        .execute(&mut *conn)
        .await?;
    let others: Vec<Uuid> = depends_on
        .iter()
        .copied()
        .filter(|d| *d != entry_id)
        .collect();
    if !others.is_empty() {
        sqlx::query(
            "INSERT INTO url_dependencies (entry_id, depends_on)
             SELECT $1, d FROM unnest($2::uuid[]) d ON CONFLICT DO NOTHING",
        )
        .bind(entry_id)
        .bind(&others)
        .execute(conn)
        .await?;
    }
    Ok(())
}

/// 再計算のジョブの中身。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Job {
    workspace_id: Uuid,
    /// 再計算する記事。None ならコンテンツタイプの公開中の記事すべて。
    entries: Option<Vec<Uuid>>,
    content_type: Option<String>,
    depth: u32,
}

async fn enqueue(pool: &PgPool, keys: &KeyRing, job: &Job) -> Result<()> {
    crate::jobs::enqueue(
        pool,
        keys,
        Some(job.workspace_id),
        JOB_KIND,
        &serde_json::to_vec(job)?,
        MAX_ATTEMPTS,
    )
    .await?;
    Ok(())
}

/// 記事に依存する記事のURLの再計算を積む（記事の公開・非公開・削除・URLの変更の後に呼ぶ）。依存元がなければ何もしない。
pub async fn schedule_dependents(
    pool: &PgPool,
    keys: &KeyRing,
    workspace_id: Uuid,
    entry_id: Uuid,
) -> Result<()> {
    schedule_dependents_at(pool, keys, workspace_id, &[entry_id], 0).await
}

async fn schedule_dependents_at(
    pool: &PgPool,
    keys: &KeyRing,
    workspace_id: Uuid,
    entry_ids: &[Uuid],
    depth: u32,
) -> Result<()> {
    let Some(db) = workspace_db::lookup(pool, workspace_id).await? else {
        return Ok(());
    };
    let mut tx = workspace_db::begin(pool, &db).await?;
    let dependents: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT entry_id FROM url_dependencies WHERE depends_on = ANY($1) AND NOT (entry_id = ANY($1))",
    )
    .bind(entry_ids)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    if dependents.is_empty() {
        return Ok(());
    }
    enqueue(
        pool,
        keys,
        &Job {
            workspace_id,
            entries: Some(dependents),
            content_type: None,
            depth,
        },
    )
    .await
}

/// コンテンツタイプの公開中の記事すべてのURLの再計算を積む（デプロイでURLの決め方が変わったとき）。
pub async fn schedule_content_type(
    pool: &PgPool,
    keys: &KeyRing,
    workspace_id: Uuid,
    content_type: &str,
) -> Result<()> {
    enqueue(
        pool,
        keys,
        &Job {
            workspace_id,
            entries: None,
            content_type: Some(content_type.to_owned()),
            depth: 0,
        },
    )
    .await
}

/// デプロイの前後で、公開URLの決め方（テンプレートか関数）が変わったコンテンツタイプ。
pub fn url_rule_changes(old: Option<&Manifest>, new: &Manifest) -> Vec<String> {
    let Some(old) = old else {
        return Vec::new();
    };
    new.content_types
        .iter()
        .filter(|ct| {
            old.content_type(&ct.name).is_some_and(|o| {
                o.url_template != ct.url_template || o.url_function != ct.url_function
            })
        })
        .map(|ct| ct.name.clone())
        .collect()
}

/// 1件の再計算の結果。
enum Recomputed {
    Unchanged,
    Changed,
    /// 公開中でない（非公開・削除）、またはコンテンツタイプがない。
    Skipped,
    /// 関数が失敗した、または別の公開中の記事とURLがぶつかる。いまのURLのまま。
    Failed(&'static str),
}

/// 公開中の記事のURLを再計算する。
async fn recompute(
    pool: &PgPool,
    db: &workspace_db::WorkspaceDb,
    engine: &Engine,
    manifest: &Manifest,
    entry_id: Uuid,
) -> Result<Recomputed> {
    let mut tx = workspace_db::begin(pool, db).await?;
    let row: Option<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT e.content_type, e.locale, e.url, v.fields::text
         FROM entries e JOIN versions v ON v.id = e.published_version_id
         WHERE e.id = $1 AND e.deleted_at IS NULL
         FOR UPDATE OF e",
    )
    .bind(entry_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((content_type, locale, current, Some(fields))) = row else {
        return Ok(Recomputed::Skipped);
    };
    let Some(ct) = manifest.content_type(&content_type) else {
        return Ok(Recomputed::Skipped);
    };
    let fields: Map<String, Value> = match serde_json::from_str(&fields)? {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    let scripts = Scripts {
        engine,
        host: Host::new(pool.clone(), db.clone()),
    };
    let (url, dependencies) =
        match publishing::compute_url(&scripts, ct, entry_id, &locale, &fields).await {
            Ok(Some(u)) => u,
            Ok(None) => return Ok(Recomputed::Skipped),
            Err(f) => return Ok(Recomputed::Failed(f.code())),
        };
    record_dependencies(&mut tx, entry_id, &dependencies).await?;
    if current.as_deref() == Some(url.as_str()) {
        tx.commit().await?;
        return Ok(Recomputed::Unchanged);
    }
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM entries WHERE url = $1 AND published_version_id IS NOT NULL AND id <> $2)",
    )
    .bind(&url)
    .bind(entry_id)
    .fetch_one(&mut *tx)
    .await?;
    if taken {
        return Ok(Recomputed::Failed("url_taken"));
    }
    sqlx::query("UPDATE entries SET url = $2 WHERE id = $1")
        .bind(entry_id)
        .bind(&url)
        .execute(&mut *tx)
        .await?;
    if let Some(from) = &current {
        sqlx::query(
            "INSERT INTO url_changes (id, entry_id, from_url, to_url, version_id) VALUES ($1, $2, $3, $4, NULL)",
        )
        .bind(Uuid::new_v4())
        .bind(entry_id)
        .bind(from)
        .bind(&url)
        .execute(&mut *tx)
        .await?;
    }
    match tx.commit().await {
        Ok(()) => Ok(Recomputed::Changed),
        // 同時に同じURLで公開された（一意の索引の違反）
        Err(_) => Ok(Recomputed::Failed("url_taken")),
    }
}

/// 再計算のジョブを1つ実行する。
pub async fn run(
    pool: &PgPool,
    keys: &KeyRing,
    engine: &Engine,
    payload: &[u8],
) -> Result<Outcome> {
    let Ok(job) = serde_json::from_slice::<Job>(payload) else {
        return Ok(Outcome::Fail("invalid_payload"));
    };
    let ws = job.workspace_id;
    let (Some(applied), Some(db)) = (
        applied_schema(pool, ws).await?,
        workspace_db::lookup(pool, ws).await?,
    ) else {
        return Ok(Outcome::Done);
    };
    let targets: Vec<Uuid> = match (&job.entries, &job.content_type) {
        (Some(ids), _) => ids.clone(),
        (None, Some(ct)) => {
            let mut tx = workspace_db::begin(pool, &db).await?;
            let ids = sqlx::query_scalar(
                "SELECT id FROM entries
                 WHERE content_type = $1 AND published_version_id IS NOT NULL AND deleted_at IS NULL
                 ORDER BY id",
            )
            .bind(ct)
            .fetch_all(&mut *tx)
            .await?;
            tx.commit().await?;
            ids
        }
        (None, None) => Vec::new(),
    };
    let mut changed = Vec::new();
    let mut failure = None;
    for id in targets {
        match recompute(pool, &db, engine, &applied.manifest, id).await? {
            Recomputed::Changed => changed.push(id),
            Recomputed::Failed(code) => failure = Some(code),
            Recomputed::Unchanged | Recomputed::Skipped => {}
        }
    }
    // URLが変わった記事: Webhookを送り、その依存元も再計算する
    for id in &changed {
        let mut tx = workspace_db::begin(pool, &db).await?;
        let entry = webhooks::entry_ref(&mut tx, *id).await?;
        tx.commit().await?;
        if let Some(entry) = entry {
            webhooks::emit(pool, keys, ws, &[EntryEvent::UrlChanged], &entry).await?;
        }
    }
    if !changed.is_empty() && job.depth < MAX_DEPTH {
        schedule_dependents_at(pool, keys, ws, &changed, job.depth + 1).await?;
    }
    // 失敗した記事があれば、ジョブごとやり直す（変わっていない記事は、やり直しても何も書かない）
    Ok(match failure {
        Some(code) => Outcome::Retry(code),
        None => Outcome::Done,
    })
}
