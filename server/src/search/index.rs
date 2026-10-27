//! 検索用データを今の状態にそろえるジョブ（08-search-and-related.md、M3）。
//!
//! - ワークスペースごとに1つにまとめて積む（公開・非公開・削除・スイッチの変更・抹消・取り込み・予算の変更・テンプレートのデプロイ・
//!   サーバーの起動のとき）。1回の実行で、次をまとめて行う。
//!   1. 対象でなくなった記事（非公開・削除・チャットボットの対象外・本文の抹消・検索の対象のフィールドがない）の行を消す（物理削除）。
//!      検索は毎回いまの状態と照合するので、消し終わる前でも結果には出ない（INV-SRC-01。照合は検索の側）。
//!   2. 公開中の版のチャンクがない・古い記事のチャンクを作り直す。テキストとモデルが同じチャンクの埋め込みは使い回す（Q58）。
//!      埋め込みは予算の範囲でだけ作る（`budget::embed`。INV-BUD-01）。足りなければ、ここで止めて15分後にやり直す
//!      （上限が変われば、すぐにやり直す）。
//! - 「決め方」（対象のフィールド・チャンクの決まり・変換器の版・モデル）が変わったら、すべての記事を見直す。
//! - 変換結果（`versions.rendered`）がまだない・古い版は、変換の作り直し（`rerender`）を待って、1分後にやり直す。
//! - LLMの接続先がなければ、検索用データは持たない（すべて消す）。

use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use super::access::{Plan, fields_key, plans};
use super::chunk::{self, Source};
use crate::budget::{self, Embedded};
use crate::config::Config;
use crate::jobs::Outcome;
use crate::keys::KeyRing;
use crate::llm::Llm;
use crate::script::CONVERTER_VERSION;
use crate::workspace_db;

pub const JOB_KIND: &str = "search_index";
const MAX_ATTEMPTS: i32 = 5;
/// 予算が足りないときに、やり直すまで待つ秒数。
pub const OVER_BUDGET_RETRY_SECONDS: f64 = 15.0 * 60.0;
/// 変換結果がまだないときに、やり直すまで待つ秒数。
const RENDER_WAIT_SECONDS: f64 = 60.0;
/// 1回に読む記事の数。
const PAGE: i64 = 50;
/// 1回の埋め込みの呼び出しに入れるチャンクの数。
const INPUTS_PER_CALL: usize = 32;
/// チャンクの決まりの版（`chunk.rs` の出力が変わる変更をしたら上げる）。
/// 2: チャンクに「検索の対象のフィールド」の決め方の要約（fields_key）を残すようにした（既にある行を書き直すため）。
const CHUNKER_VERSION: &str = "2";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Job {
    workspace_id: Uuid,
}

/// ワークスペースのジョブを積む（まだ動いていない同じジョブがあれば、それにまとめて、実行の時刻を `delay_seconds` 後にする）。
pub async fn schedule(
    pool: &PgPool,
    keys: &KeyRing,
    workspace_id: Uuid,
    delay_seconds: f64,
) -> Result<()> {
    crate::jobs::enqueue_debounced(
        pool,
        keys,
        Some(workspace_id),
        JOB_KIND,
        &serde_json::to_vec(&Job { workspace_id })?,
        MAX_ATTEMPTS,
        &format!("{JOB_KIND}:{workspace_id}"),
        delay_seconds,
    )
    .await
}

/// スキーマを適用済みのすべてのワークスペースに積む（サーバーの起動のとき）。
pub async fn schedule_all(pool: &PgPool, keys: &KeyRing) -> Result<()> {
    let ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM cms.workspaces WHERE schema_version IS NOT NULL")
            .fetch_all(pool)
            .await?;
    for id in ids {
        schedule(pool, keys, id, 0.0).await?;
    }
    Ok(())
}

fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 公開中の版の、検索の対象のフィールドの値。
struct Candidate {
    entry_id: Uuid,
    content_type: String,
    locale: String,
    version_id: Uuid,
    fields: Value,
    rendered: Option<Value>,
    rendered_with: Option<String>,
}

/// 候補の行（記事のID・タイプ・ロケール・版のID・フィールドのJSON・変換結果のJSON・変換器の版）。
type CandidateRow = (
    Uuid,
    String,
    String,
    Uuid,
    String,
    Option<String>,
    Option<String>,
);

/// 対象の記事の条件（`e` は entries、`v` は公開中の版）。
const ELIGIBLE: &str = "e.deleted_at IS NULL AND e.use_for_chatbot AND v.erased_at IS NULL
     AND v.fields IS NOT NULL AND e.content_type = ANY($1)";

/// ジョブを1つ実行する。
pub async fn run(
    pool: &PgPool,
    keys: &KeyRing,
    config: &Config,
    egress: Option<&crate::egress::Egress>,
    payload: &[u8],
) -> Result<Outcome> {
    let Ok(job) = serde_json::from_slice::<Job>(payload) else {
        return Ok(Outcome::Fail("invalid_payload"));
    };
    let ws = job.workspace_id;
    let (Some(applied), Some(db)) = (
        crate::content::applied_schema(pool, ws).await?,
        workspace_db::lookup(pool, ws).await?,
    ) else {
        return Ok(Outcome::Done);
    };
    let llm = match (&config.llm, egress) {
        (Some(c), Some(e)) => Llm::new(e.clone(), c.clone()),
        _ => {
            // LLMの接続先がなければ、検索用データは持たない
            let mut tx = workspace_db::begin(pool, &db).await?;
            sqlx::query("DELETE FROM search_chunks")
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM search_state")
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return Ok(Outcome::Done);
        }
    };
    let model = llm.config().embedding_model.clone();
    let dims = i32::try_from(llm.config().embedding_dimensions).unwrap_or(i32::MAX);
    let plans: HashMap<String, Plan> = plans(&applied.manifest).into_iter().collect();
    let types: Vec<String> = plans.keys().cloned().collect();
    let mut sorted: Vec<(&String, &Plan)> = plans.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let plan_key = sha256_hex(&serde_json::to_string(&(
        &sorted,
        CHUNKER_VERSION,
        chunk::MAX_CHARS,
        chunk::MIN_CHARS,
        CONVERTER_VERSION,
        &model,
        dims,
    ))?);

    // 1. 対象でなくなった記事の行を消す（物理削除）
    let mut tx = workspace_db::begin(pool, &db).await?;
    sqlx::query(crate::sql!(
        "DELETE FROM search_chunks c WHERE NOT EXISTS (
           SELECT 1 FROM entries e JOIN versions v ON v.id = e.published_version_id
           WHERE e.id = c.entry_id AND {ELIGIBLE})"
    ))
    .bind(&types)
    .execute(&mut *tx)
    .await?;
    let stored: Option<String> = sqlx::query_scalar("SELECT plan_key FROM search_state")
        .fetch_optional(&mut *tx)
        .await?;
    tx.commit().await?;
    // 決め方が変わっていれば、すべての記事を見直す
    let full = stored.as_deref() != Some(plan_key.as_str());

    // 2. チャンクがない・古い記事を作り直す
    let mut after = Uuid::nil();
    let mut waiting_for_render = false;
    loop {
        let mut tx = workspace_db::begin(pool, &db).await?;
        let rows: Vec<CandidateRow> =
            sqlx::query_as(crate::sql!(
                "SELECT e.id, e.content_type, e.locale, v.id, v.fields::text, v.rendered::text, v.rendered_with
                 FROM entries e JOIN versions v ON v.id = e.published_version_id
                 WHERE {ELIGIBLE} AND e.id > $2
                   AND ($3 OR NOT EXISTS (
                     SELECT 1 FROM search_chunks c
                     WHERE c.entry_id = e.id AND c.version_id = v.id AND c.model = $4 AND c.dimensions = $5))
                 ORDER BY e.id
                 LIMIT $6"
            ))
            .bind(&types)
            .bind(after)
            .bind(full)
            .bind(&model)
            .bind(dims)
            .bind(PAGE)
            .fetch_all(&mut *tx)
            .await?;
        tx.commit().await?;
        let Some(last) = rows.last() else { break };
        after = last.0;
        for (entry_id, content_type, locale, version_id, fields, rendered, rendered_with) in rows {
            // 本文などは JSON の文字列で読み、ここで解釈する
            let candidate = Candidate {
                entry_id,
                content_type,
                locale,
                version_id,
                fields: serde_json::from_str(&fields)?,
                rendered: rendered.as_deref().map(serde_json::from_str).transpose()?,
                rendered_with,
            };
            match index_entry(
                pool, config, &llm, &db, ws, &plans, &candidate, &model, dims,
            )
            .await?
            {
                Step::Done => {}
                Step::WaitForRender => waiting_for_render = true,
                Step::OverBudget => {
                    schedule(pool, keys, ws, OVER_BUDGET_RETRY_SECONDS).await?;
                    return Ok(Outcome::Done);
                }
                Step::Failed(code) => return Ok(Outcome::Retry(code)),
            }
        }
    }
    if waiting_for_render {
        schedule(pool, keys, ws, RENDER_WAIT_SECONDS).await?;
    } else if full {
        let mut tx = workspace_db::begin(pool, &db).await?;
        sqlx::query(
            "INSERT INTO search_state (id, plan_key) VALUES (true, $1)
             ON CONFLICT (id) DO UPDATE SET plan_key = $1",
        )
        .bind(&plan_key)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
    }
    Ok(Outcome::Done)
}

enum Step {
    Done,
    WaitForRender,
    OverBudget,
    Failed(&'static str),
}

/// 1つの記事のチャンクを作り直す。
#[allow(clippy::too_many_arguments)]
async fn index_entry(
    pool: &PgPool,
    config: &Config,
    llm: &Llm,
    db: &workspace_db::WorkspaceDb,
    ws: Uuid,
    plans: &HashMap<String, Plan>,
    c: &Candidate,
    model: &str,
    dims: i32,
) -> Result<Step> {
    let Some(plan) = plans.get(&c.content_type) else {
        return Ok(Step::Done);
    };
    let text_of = |name: &str| c.fields.get(name).and_then(Value::as_str);
    let mut sources = Vec::new();
    for (name, richtext) in &plan.fields {
        if *richtext {
            if c.fields.get(name).is_none_or(Value::is_null) {
                continue;
            }
            // 変換結果がない・古い版は、作り直しを待つ
            let markdown = c
                .rendered
                .as_ref()
                .and_then(|r| r.get(name))
                .and_then(|r| r.get("markdown"))
                .and_then(Value::as_str);
            match markdown {
                Some(md) if c.rendered_with.as_deref() == Some(CONVERTER_VERSION) => {
                    sources.push(Source::Markdown(md))
                }
                _ => return Ok(Step::WaitForRender),
            }
        } else if let Some(t) = text_of(name) {
            sources.push(Source::Text(t));
        }
    }
    let title = plan.title.as_deref().and_then(text_of);
    let chunks = chunk::chunks(title, &sources);
    let inputs: Vec<String> = chunks.iter().map(chunk::embedding_input).collect();
    let hashes: Vec<String> = inputs.iter().map(|s| sha256_hex(s)).collect();

    // 今のチャンクと同じなら、書き直さない
    let mut tx = workspace_db::begin(pool, db).await?;
    let key = fields_key(plan);
    let current: Vec<(String, Uuid, String)> = sqlx::query_as(
        "SELECT content_hash, version_id, fields_key FROM search_chunks
         WHERE entry_id = $1 AND model = $2 AND dimensions = $3 ORDER BY seq",
    )
    .bind(c.entry_id)
    .bind(model)
    .bind(dims)
    .fetch_all(&mut *tx)
    .await?;
    if !current.is_empty()
        && current
            .iter()
            .all(|(_, v, k)| *v == c.version_id && *k == key)
        && current.iter().map(|(h, _, _)| h).eq(hashes.iter())
    {
        return Ok(Step::Done);
    }
    // テキストとモデルが同じ埋め込みは使い回す
    let reused: Vec<(String, Vec<f32>)> = sqlx::query_as(
        "SELECT DISTINCT ON (content_hash) content_hash, embedding FROM search_chunks
         WHERE content_hash = ANY($1) AND model = $2 AND dimensions = $3",
    )
    .bind(&hashes)
    .bind(model)
    .bind(dims)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let mut vectors: HashMap<String, Vec<f32>> = reused.into_iter().collect();
    let missing: Vec<usize> = (0..inputs.len())
        .filter(|i| !vectors.contains_key(&hashes[*i]))
        .collect();
    for group in missing.chunks(INPUTS_PER_CALL) {
        let batch: Vec<String> = group.iter().map(|i| inputs[*i].clone()).collect();
        match budget::embed(pool, config, llm, ws, &batch).await? {
            Embedded::Done(e) => {
                for (i, v) in group.iter().zip(e.vectors) {
                    vectors.insert(hashes[*i].clone(), v);
                }
            }
            Embedded::OverBudget => return Ok(Step::OverBudget),
            Embedded::Failed(e) => return Ok(Step::Failed(e.code())),
        }
    }

    // 書き直す（その間に記事が変わっていたら、何もしない。次の実行で見直す）
    let mut tx = workspace_db::begin(pool, db).await?;
    let still: Option<Uuid> = sqlx::query_scalar(
        "SELECT published_version_id FROM entries WHERE id = $1 AND deleted_at IS NULL AND use_for_chatbot FOR UPDATE",
    )
    .bind(c.entry_id)
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    if still != Some(c.version_id) {
        return Ok(Step::Done);
    }
    sqlx::query("DELETE FROM search_chunks WHERE entry_id = $1")
        .bind(c.entry_id)
        .execute(&mut *tx)
        .await?;
    for (seq, (chunk, hash)) in chunks.iter().zip(&hashes).enumerate() {
        sqlx::query(
            "INSERT INTO search_chunks
               (id, entry_id, version_id, locale, seq, anchor, breadcrumb, content, content_hash, model, dimensions, embedding,
                fields_key)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
        )
        .bind(Uuid::new_v4())
        .bind(c.entry_id)
        .bind(c.version_id)
        .bind(&c.locale)
        .bind(i32::try_from(seq).unwrap_or(i32::MAX))
        .bind(&chunk.anchor)
        .bind(&chunk.breadcrumb)
        .bind(&chunk.text)
        .bind(hash)
        .bind(model)
        .bind(dims)
        .bind(&vectors[hash])
        .bind(&key)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Step::Done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::manifest::Manifest;

    #[test]
    fn title_is_used_only_when_its_field_is_searchable() {
        let mut m: Manifest = serde_json::from_value(serde_json::json!({
            "locales": ["ja"], "defaultLocale": "ja",
            "body": { "nodes": [], "marks": [], "embedProviders": [] },
            "contentTypes": [{
                "name": "post", "urlTemplate": "https://x/{slug}", "listLimit": 10, "titleField": "title",
                "fields": [
                    { "name": "title", "type": "text" },
                    { "name": "slug", "type": "slug" },
                    { "name": "body", "type": "richtext", "searchable": true }
                ]
            }, {
                "name": "page", "urlTemplate": "https://x/p/{slug}", "listLimit": 10,
                "fields": [ { "name": "slug", "type": "slug" } ]
            }]
        }))
        .unwrap();
        let p = plans(&m);
        assert_eq!(p.len(), 1, "検索の対象のフィールドのないタイプは対象外");
        assert_eq!(p[0].1.fields, vec![("body".to_owned(), true)]);
        assert_eq!(
            p[0].1.title, None,
            "タイトルのフィールドが対象でなければ、パンくずに入れない"
        );
        m.content_types[0].fields[0].searchable = true;
        let p = plans(&m);
        assert_eq!(p[0].1.title.as_deref(), Some("title"));
        assert_eq!(
            p[0].1.fields,
            vec![("title".to_owned(), false), ("body".to_owned(), true)]
        );
    }
}
