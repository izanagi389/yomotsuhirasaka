//! 検索用データを今の状態にそろえるジョブ（08-search-and-related.md、M3）。
//!
//! - ワークスペースごとに1つにまとめて積む（公開・非公開・削除・スイッチの変更・抹消・取り込み・予算の変更・テンプレートのデプロイ・
//!   サーバーの起動のとき）。1回の実行で、次をまとめて行う。
//!   1. 対象でなくなった記事（非公開・削除・チャットボットの対象外・本文の抹消・検索の対象のフィールドがない）の行を消す（物理削除）。
//!      検索は毎回いまの状態と照合するので、消し終わる前でも結果には出ない（INV-SRC-01。照合は検索の側）。
//!      どのチャンクにも使われなくなった埋め込みも消す（本文を抹消した版の埋め込みを含む。Q134）。
//!   2. 公開中の版のチャンクがない・古い記事のチャンクを作り直す。チャンク（本文）は、LLMの接続先がなくても、予算が尽きていても作る
//!      （キーワード検索に使うため）。
//!   3. LLMの接続先があれば、今のモデルの埋め込みがないテキストの埋め込みを作る。埋め込みはテキスト（content_hash）と
//!      モデルごとに1つで、同じテキストのチャンクは使い回す（Q58。`search_embeddings`）。埋め込みは予算の範囲でだけ作る
//!      （`budget::embed`。INV-BUD-01）。足りなければ、ここで止めて15分後にやり直す（上限が変われば、すぐにやり直す）。
//!   4. 記事全体のベクトルを作り直し、作り直した記事の関連記事を計算する（`related`）。
//! - 「決め方」（対象のフィールド・チャンクの決まり・変換器の版）が変わったら、すべての記事を見直す。
//! - 変換結果（`versions.rendered`）がまだない・古い版は、変換の作り直し（`rerender`）を待って、1分後にやり直す。

use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use super::access::{Plan, fields_key, plans};
use super::chunk::{self, Source};
use super::switch;
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
        sqlx::query_scalar("SELECT id FROM yomi.workspaces WHERE schema_version IS NOT NULL")
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
    sqlx::query(
        "DELETE FROM search_embeddings x
         WHERE NOT EXISTS (SELECT 1 FROM search_chunks c WHERE c.content_hash = x.content_hash)",
    )
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
                     SELECT 1 FROM search_chunks c WHERE c.entry_id = e.id AND c.version_id = v.id))
                 ORDER BY e.id
                 LIMIT $4"
            ))
            .bind(&types)
            .bind(after)
            .bind(full)
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
            if !index_entry(pool, &db, &plans, &candidate).await? {
                waiting_for_render = true;
            }
        }
    }

    // 3. 埋め込みを作る（LLMの接続先がなければ、チャンクだけ）。モデルの切り替えの途中なら、旧モデルの分を先に作る（`switch`）
    // 埋め込みの接続先とモデルは、ワークスペースの上書きを含めたもの（`workspace_llm::embedding`）
    let setup = crate::workspace_llm::embedding(pool, keys, config, ws).await?;
    if let (Some(setup), Some(egress)) = (&setup, egress) {
        let llm_config = &setup.llm;
        let mut tx = workspace_db::begin(pool, &db).await?;
        let rec = switch::recorded(&mut tx).await?;
        tx.commit().await?;
        let plan = switch::plan(llm_config, &rec, &setup.started);
        prepare_models(pool, &db, llm_config, &rec, &plan).await?;
        for target in &plan.embed {
            let llm = Llm::new(egress.clone(), target.clone());
            let dims = i32::try_from(target.embedding_dimensions).unwrap_or(i32::MAX);
            match embed_chunks(pool, config, &llm, &db, ws, &target.embedding_model, dims).await? {
                Step::Done => {}
                Step::OverBudget => {
                    set_paused(pool, &db, true).await?;
                    schedule(pool, keys, ws, OVER_BUDGET_RETRY_SECONDS).await?;
                    return Ok(Outcome::Done);
                }
                // ワークスペースが開始した切り替え先の次元数が合わなければ、切り替えを止めて理由を記録する（前のモデルで続ける）
                Step::Failed(code)
                    if code == "dimension_mismatch"
                        && crate::workspace_llm::embedding_switch_failed(
                            pool,
                            ws,
                            &(target.embedding_model.clone(), target.embedding_dimensions),
                            code,
                        )
                        .await? => {}
                Step::Failed(code) => return Ok(Outcome::Retry(code)),
            }
        }
        set_paused(pool, &db, false).await?;
        if !waiting_for_render && let Some(target) = &plan.target {
            switch_if_complete(pool, &db, ws, target).await?;
        }
        // 4. 記事のベクトルと関連記事（今使っているモデルで。外部とは通信しない）
        let mut tx = workspace_db::begin(pool, &db).await?;
        let serving = switch::serving(Some(llm_config), &mut tx).await?;
        tx.commit().await?;
        if let (Some(s), Some(vs)) = (serving, super::vector_schema(pool).await?) {
            super::related::refresh(
                pool,
                &db,
                &applied.manifest,
                &vs,
                &s.embedding_model,
                s.embedding_dimensions,
            )
            .await?;
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
    OverBudget,
    Failed(&'static str),
}

/// 1つの記事のチャンクを作り直す（埋め込みは、テキストの要約で結び付くので、ここでは作らない）。
/// 変換結果がまだない・古いために作れなければ false。
async fn index_entry(
    pool: &PgPool,
    db: &workspace_db::WorkspaceDb,
    plans: &HashMap<String, Plan>,
    c: &Candidate,
) -> Result<bool> {
    let Some(plan) = plans.get(&c.content_type) else {
        return Ok(true);
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
                _ => return Ok(false),
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
        "SELECT content_hash, version_id, fields_key FROM search_chunks WHERE entry_id = $1 ORDER BY seq",
    )
    .bind(c.entry_id)
    .fetch_all(&mut *tx)
    .await?;
    if !current.is_empty()
        && current
            .iter()
            .all(|(_, v, k)| *v == c.version_id && *k == key)
        && current.iter().map(|(h, _, _)| h).eq(hashes.iter())
    {
        return Ok(true);
    }
    // 書き直す（その間に記事が変わっていたら、何もしない。次の実行で見直す）
    let still: Option<Uuid> = sqlx::query_scalar(
        "SELECT published_version_id FROM entries WHERE id = $1 AND deleted_at IS NULL AND use_for_chatbot FOR UPDATE",
    )
    .bind(c.entry_id)
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    if still != Some(c.version_id) {
        return Ok(true);
    }
    sqlx::query("DELETE FROM search_chunks WHERE entry_id = $1")
        .bind(c.entry_id)
        .execute(&mut *tx)
        .await?;
    for (seq, (chunk, hash)) in chunks.iter().zip(&hashes).enumerate() {
        sqlx::query(
            "INSERT INTO search_chunks
               (id, entry_id, version_id, locale, seq, anchor, breadcrumb, content, content_hash, fields_key)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
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
        .bind(&key)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(true)
}

/// ワークスペースのモデルの記録を用意し（なければ今のモデルで作る）、残さないモデルの埋め込みを消す。
async fn prepare_models(
    pool: &PgPool,
    db: &workspace_db::WorkspaceDb,
    llm: &crate::config::LlmConfig,
    rec: &switch::Recorded,
    plan: &switch::Plan,
) -> Result<()> {
    let (models, dims): (Vec<String>, Vec<i32>) = switch::kept(llm, rec, plan)
        .into_iter()
        .map(|(m, d)| (m, i32::try_from(d).unwrap_or(i32::MAX)))
        .unzip();
    let mut tx = workspace_db::begin(pool, db).await?;
    sqlx::query(
        "INSERT INTO search_models (id, active_model, active_dimensions) VALUES (true, $1, $2)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(&plan.active.0)
    .bind(i32::try_from(plan.active.1).unwrap_or(i32::MAX))
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM search_embeddings x
         WHERE (x.model, x.dimensions) NOT IN (SELECT * FROM unnest($1::text[], $2::integer[]))",
    )
    .bind(&models)
    .bind(&dims)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// 予算が尽きて止まったか（進んだら消す）を記録する。
async fn set_paused(pool: &PgPool, db: &workspace_db::WorkspaceDb, paused: bool) -> Result<()> {
    let mut tx = workspace_db::begin(pool, db).await?;
    sqlx::query(if paused {
        "UPDATE search_models SET paused_at = coalesce(paused_at, now())"
    } else {
        "UPDATE search_models SET paused_at = NULL WHERE paused_at IS NOT NULL"
    })
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// 切り替え先のモデルの埋め込みが、すべてのチャンクにそろっていれば切り替える（1つの文で、そろったことの確認と一緒に行う）。
async fn switch_if_complete(
    pool: &PgPool,
    db: &workspace_db::WorkspaceDb,
    ws: Uuid,
    target: &switch::ModelId,
) -> Result<()> {
    let mut tx = workspace_db::begin(pool, db).await?;
    let switched = sqlx::query(
        "UPDATE search_models
         SET previous_model = active_model, previous_dimensions = active_dimensions,
             active_model = $1, active_dimensions = $2, switched_at = now(), paused_at = NULL
         WHERE (active_model, active_dimensions) <> ($1, $2)
           AND NOT EXISTS (
             SELECT 1 FROM search_chunks c
             WHERE NOT EXISTS (SELECT 1 FROM search_embeddings x
                               WHERE x.content_hash = c.content_hash AND x.model = $1 AND x.dimensions = $2))",
    )
    .bind(&target.0)
    .bind(i32::try_from(target.1).unwrap_or(i32::MAX))
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    if switched > 0 {
        tracing::info!(workspace = %ws, "switched the embedding model of the workspace");
    }
    Ok(())
}

/// 埋め込みを作る対象のテキストの行（テキストの要約・アンカー・パンくず・本文）。
type UnembeddedRow = (String, Option<String>, Vec<String>, String);

/// 今のモデルの埋め込みがないテキストの埋め込みを、予算の範囲で作る。
async fn embed_chunks(
    pool: &PgPool,
    config: &Config,
    llm: &Llm,
    db: &workspace_db::WorkspaceDb,
    ws: Uuid,
    model: &str,
    dims: i32,
) -> Result<Step> {
    let mut after = String::new();
    loop {
        let mut tx = workspace_db::begin(pool, db).await?;
        let rows: Vec<UnembeddedRow> = sqlx::query_as(
            "SELECT DISTINCT ON (c.content_hash) c.content_hash, c.anchor, c.breadcrumb, c.content
             FROM search_chunks c
             WHERE c.content_hash > $1
               AND NOT EXISTS (SELECT 1 FROM search_embeddings x
                               WHERE x.content_hash = c.content_hash AND x.model = $2 AND x.dimensions = $3)
             ORDER BY c.content_hash
             LIMIT $4",
        )
        .bind(&after)
        .bind(model)
        .bind(dims)
        .bind(i64::try_from(INPUTS_PER_CALL).unwrap_or(i64::MAX))
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        let Some(last) = rows.last() else {
            return Ok(Step::Done);
        };
        after = last.0.clone();
        let inputs: Vec<String> = rows
            .iter()
            .map(|(_, anchor, breadcrumb, text)| {
                chunk::embedding_input(&chunk::Chunk {
                    anchor: anchor.clone(),
                    breadcrumb: breadcrumb.clone(),
                    text: text.clone(),
                })
            })
            .collect();
        let vectors = match budget::embed(pool, config, llm, ws, &inputs).await? {
            Embedded::Done(e) => e.vectors,
            Embedded::OverBudget => return Ok(Step::OverBudget),
            Embedded::Failed(e) => return Ok(Step::Failed(e.code())),
        };
        let mut tx = workspace_db::begin(pool, db).await?;
        for ((hash, ..), v) in rows.iter().zip(vectors) {
            sqlx::query(
                "INSERT INTO search_embeddings (content_hash, model, dimensions, embedding)
                 VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
            )
            .bind(hash)
            .bind(model)
            .bind(dims)
            .bind(&v)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
    }
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
