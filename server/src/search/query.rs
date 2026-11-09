//! サイト内検索の実行（08-search-and-related.md、M3）。配信APIの `search` から呼ぶ。
//!
//! - 検索の方法は3つ。既定は2つを混ぜる（`Hybrid`）。ベクトル検索だけ（`Vector`）、キーワード検索だけ（`Keyword`）も選べる。
//!   混ぜるときは、それぞれの上位 `max(first × 3, 50)` 件のチャンクを取り、同じ記事は一番上の順位だけを使って、
//!   RRF（`1 / (60 + 順位)` の和）で並べる。
//! - 片方が使えない（LLMの接続先・拡張がない、予算が尽きた）ときは、その状態を応答で示し、使える側だけで結果を返す。
//!   キーワード検索だけを選んだときは、埋め込みを作らない。使わなかった側の状態は、設定の上で使えるかを示す（予算は確かめない）。
//! - 検索語は、外部に送る前にマスクする（`crate::mask`。Q169）。長すぎる検索語は切り詰める。キーワード検索もマスクした検索語を使う。
//! - 検索語の埋め込みは、呼び出し側が決めた予算から出す（`Services::query_budget`。記事の埋め込みの分を減らさない）。モデルは、ワークスペースが今使っているもの
//!   （モデルの切り替えの途中は旧モデル。使っているモデルが設定になければ、ベクトル検索は無効）。
//! - 検索語の埋め込みは、メモリの中に10分だけ持つ。キーは（ワークスペース、モデル、マスクした検索語）の SHA-256 で、
//!   検索語そのものは持たない（原則7）。持つ数には上限がある。
//! - 見せてよいチャンクは、共通の層（`access`）の条件で、検索のたびに今の状態と照合して選ぶ。
//! - キーワード検索は、検索語を空白で区切った語（8個まで）を、すべて含むチャンクだけを返す（pg_bigm の索引を使う LIKE。
//!   大文字・小文字は区別しない）。並びは pg_bigm の類似度の順。
//! - 1つの記事からは、いちばん合うチャンクを1件だけ返す。類似度の足切りはしない（サイトに任せる）。
//! - `score` の意味は方法ごとに違う（混ぜたとき RRF の点、ベクトル検索はコサイン類似度、キーワード検索は pg_bigm の類似度）。
//!   同じ応答の中で並びを比べるのにだけ使える。
//! - 抜粋は、チャンクの本文（検索の対象のフィールドから作ったもの）をプレーンテキストにして200文字まで。HTMLは返さない。
//!   キーワードで当たったチャンクは、最初に一致した箇所のまわりを切り出し、一致した箇所の位置（UTF-16 の単位）を返す。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use sha2::{Digest as _, Sha256};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::access;
use crate::budget::{self, Embedded};
use crate::config::{Config, LlmConfig};
use crate::db::Ident;
use crate::egress::Egress;
use crate::llm::Llm;
use crate::schema::manifest::Manifest;

/// `first` の既定と上限。
pub const DEFAULT_FIRST: i64 = 10;
pub const MAX_FIRST: i64 = 50;
/// 抜粋の文字数の上限。
pub const EXCERPT_CHARS: usize = 200;
/// 検索語の文字数の上限（超える分は使わない。費用を抑えるため）。
pub const MAX_QUERY_CHARS: usize = 500;
/// キーワード検索に使う語の数の上限（超える分は使わない）。
pub const MAX_TERMS: usize = 8;
/// RRF の定数 k。
const RRF_K: f64 = 60.0;
/// 混ぜるときに、片側から取るチャンクの数の下限（`first × 3` がこれより少なければ、これだけ取る）。
const MIN_CANDIDATES: i64 = 50;
/// 抜粋で、最初に一致した箇所の前に残す文字数。
const CONTEXT_BEFORE: usize = 60;
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const CACHE_MAX: usize = 1000;

/// キャッシュの1件（入れた時刻と、検索語の埋め込み）。
type Cached = (Instant, Vec<f32>);

/// 検索語の埋め込みのキャッシュ（サーバーのプロセスごと）。
#[derive(Default)]
pub struct Cache {
    map: Mutex<HashMap<[u8; 32], Cached>>,
}

impl Cache {
    fn get(&self, key: &[u8; 32]) -> Option<Vec<f32>> {
        let map = self.map.lock().expect("search cache lock");
        map.get(key)
            .filter(|(at, _)| at.elapsed() < CACHE_TTL)
            .map(|(_, v)| v.clone())
    }

    fn put(&self, key: [u8; 32], vector: Vec<f32>) {
        let mut map = self.map.lock().expect("search cache lock");
        map.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
        if map.len() >= CACHE_MAX
            && let Some(oldest) = map.iter().min_by_key(|(_, (at, _))| *at).map(|(k, _)| *k)
        {
            map.remove(&oldest);
        }
        map.insert(key, (Instant::now(), vector));
    }
}

/// ベクトル検索・キーワード検索が使えるか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Available,
    /// 予算が尽きている（ベクトル検索だけ）。
    BudgetExhausted,
    /// 使えない（LLMの接続先や拡張がない）。
    Disabled,
}

impl Availability {
    /// GraphQL の列挙の値。
    pub fn name(self) -> &'static str {
        match self {
            Self::Available => "AVAILABLE",
            Self::BudgetExhausted => "BUDGET_EXHAUSTED",
            Self::Disabled => "DISABLED",
        }
    }
}

/// 検索の方法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Hybrid,
    Vector,
    Keyword,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Hybrid, Mode::Vector, Mode::Keyword];

    /// GraphQL の列挙の値。
    pub fn name(self) -> &'static str {
        match self {
            Self::Hybrid => "HYBRID",
            Self::Vector => "VECTOR",
            Self::Keyword => "KEYWORD",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.name() == name)
    }
}

/// 抜粋の中で、検索語と一致した箇所（UTF-16 の単位の位置と長さ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    pub start: usize,
    pub length: usize,
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub entry_id: Uuid,
    pub locale: String,
    /// 記事の公開URL（アンカーは付けない）。
    pub url: Option<String>,
    pub anchor: Option<String>,
    pub breadcrumb: Vec<String>,
    pub excerpt: String,
    /// キーワードで当たったときの一致した箇所（ベクトル検索だけで当たったときは空）。
    pub matches: Vec<Match>,
    pub score: f64,
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub hits: Vec<Hit>,
    pub vector: Availability,
    pub keyword: Availability,
}

/// 検索に要る、ワークスペースの外のもの（予算・埋め込みの呼び出し・キャッシュ）。
pub struct Services<'a> {
    pub pool: &'a sqlx::PgPool,
    pub config: &'a Config,
    /// ワークスペースの埋め込みの接続先（上書きを含む。`workspace_llm::embedding`）。なければベクトル検索は使えない。
    pub embedding: Option<&'a crate::config::LlmConfig>,
    pub egress: Option<&'a Egress>,
    pub cache: &'a Cache,
    /// 検索語の埋め込みを出す予算（訪問者の検索と質問は [`budget::Kind::VisitorEmbedding`]、矛盾の検知は埋め込みの作成）。
    pub query_budget: budget::Kind,
}

/// 検索の失敗（相手の応答の本文や検索語を含めない）。
#[derive(Debug)]
pub struct Unavailable(pub &'static str);

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "search is unavailable ({})", self.0)
    }
}

impl std::error::Error for Unavailable {}

/// 検索の結果の行（記事のID・ロケール・アンカー・パンくず・本文・点数）。
type HitRow = (Uuid, String, Option<String>, Vec<String>, String, f64);

/// 検索する。`tx` は、ワークスペースのロールとスキーマに切り替えた読み取りのトランザクション。
#[allow(clippy::too_many_arguments)]
pub async fn search(
    services: &Services<'_>,
    tx: &mut Transaction<'static, Postgres>,
    workspace_id: Uuid,
    manifest: &Manifest,
    text: &str,
    locale: &str,
    first: i64,
    mode: Mode,
) -> Result<Outcome> {
    // 設定の上で、それぞれの検索が使えるか
    // ベクトル検索は、ワークスペースが今使っているモデルで行う（モデルの切り替えの途中は旧モデル。`super::switch`）
    let serving = super::switch::serving(services.embedding, tx).await?;
    let vector_setup = match (serving, services.egress) {
        (Some(c), Some(e)) => super::vector_schema(&mut **tx).await?.map(|vs| (c, e, vs)),
        _ => None,
    };
    let keyword_schema = super::keyword_schema(tx).await?;
    let available = |yes: bool| {
        if yes {
            Availability::Available
        } else {
            Availability::Disabled
        }
    };
    let mut out = Outcome {
        hits: Vec::new(),
        vector: available(vector_setup.is_some()),
        keyword: available(keyword_schema.is_some()),
    };
    let first = first.clamp(0, MAX_FIRST);
    let text: String = text.trim().chars().take(MAX_QUERY_CHARS).collect();
    let masked = crate::mask::mask(&text);
    if first == 0 || masked.trim().is_empty() {
        return Ok(out);
    }
    // 混ぜるときは、片側から多めに取る。1つの方法だけなら、記事ごとに1件にして first 件
    let (limit, per_entry) = match mode {
        Mode::Hybrid => ((first * 3).max(MIN_CANDIDATES), false),
        _ => (first, true),
    };
    let (types, keys) = access::current_keys(manifest);
    let target = Target {
        types: &types,
        keys: &keys,
        locale,
        untranslated: None,
        limit,
        per_entry,
    };

    let mut vector_rows = Vec::new();
    if mode != Mode::Keyword
        && let Some((llm_config, egress, vs)) = &vector_setup
    {
        match query_vector(services, workspace_id, llm_config, egress, &masked).await? {
            Some(vector) => {
                // ベクトル検索は HNSW の索引でチャンクのまま近い順に取り、記事ごとに1件にする（索引を使うため、SQL では
                // 記事ごとにまとめない）
                let n = (first * 3).max(MIN_CANDIDATES);
                let rows = vector_candidates(tx, &target, vs, llm_config, &vector, n).await?;
                vector_rows = if mode == Mode::Vector {
                    best_per_entry(rows)
                } else {
                    rows
                };
            }
            None => out.vector = Availability::BudgetExhausted,
        }
    }
    let terms = terms(&masked);
    let mut keyword_rows = Vec::new();
    if mode != Mode::Vector
        && let Some(bs) = &keyword_schema
    {
        keyword_rows = keyword_candidates(tx, &target, bs, &terms).await?;
    }

    let vector_hit = |row: HitRow| {
        let (entry_id, locale, anchor, breadcrumb, content, score) = row;
        Hit {
            entry_id,
            locale,
            url: None,
            anchor,
            breadcrumb,
            excerpt: excerpt(&content),
            matches: Vec::new(),
            score,
        }
    };
    let keyword_hit = |row: HitRow| {
        let (entry_id, locale, anchor, breadcrumb, content, score) = row;
        let (excerpt, matches) = excerpt_around(&content, &terms);
        Hit {
            entry_id,
            locale,
            url: None,
            anchor,
            breadcrumb,
            excerpt,
            matches,
            score,
        }
    };
    let first = usize::try_from(first).unwrap_or(0);
    out.hits = match mode {
        Mode::Vector => vector_rows.into_iter().map(vector_hit).collect(),
        Mode::Keyword => keyword_rows.into_iter().map(keyword_hit).collect(),
        Mode::Hybrid => fuse(
            best_per_entry(vector_rows),
            best_per_entry(keyword_rows),
            first,
        )
        .into_iter()
        .map(|(score, from)| {
            // キーワードで当たった記事は、そのチャンク（一致した箇所が分かる）を見せる
            let mut hit = match from {
                Source::Keyword(row) => keyword_hit(row),
                Source::Vector(row) => vector_hit(row),
            };
            hit.score = score;
            hit
        })
        .collect(),
    };
    out.hits.truncate(first);
    // 記事の公開URL（サイトがリンクを作るため。配信APIで公開している値）
    let ids: Vec<Uuid> = out.hits.iter().map(|h| h.entry_id).collect();
    let urls: HashMap<Uuid, Option<String>> = sqlx::query_as::<_, (Uuid, Option<String>)>(
        "SELECT id, url FROM entries WHERE id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .collect();
    for hit in &mut out.hits {
        hit.url = urls.get(&hit.entry_id).cloned().flatten();
    }
    Ok(out)
}

/// 質問応答の根拠にする断片（チャンク1つ。09-qa-chatbot.md）。
#[derive(Debug, Clone, PartialEq)]
pub struct Fragment {
    pub entry_id: Uuid,
    pub locale: String,
    pub anchor: Option<String>,
    pub breadcrumb: Vec<String>,
    /// チャンクの本文（変換器の Markdown）。
    pub content: String,
    /// 並べた点数（RRF）と、もとの点数（ベクトル検索のコサイン類似度・キーワード検索の点数。その方式の候補でなければ None）。
    /// 管理画面の回答のテスト画面で見せる（09-qa-chatbot.md「根拠と判定」）。
    pub score: f64,
    pub vector_similarity: Option<f64>,
    pub keyword_score: Option<f64>,
}

/// 質問に近い断片を、チャンクのまま `k` 件まで選ぶ（質問応答の根拠。M4）。
/// - 見せてよいチャンクの条件は検索と同じ（共通の層 `access`。質問のたびに今の状態と照合する）。
/// - ベクトル検索とキーワード検索の両方の候補を、検索の混ぜ方（RRF）で、記事ごとにまとめずに並べる。
/// - ベクトル検索の候補のうち、コサイン類似度が `min_similarity` に届かないものは外す（ゆるい足切り。最終の判断はLLM。Q15）。
/// - 質問のロケールが既定のロケールでなければ、既定のロケールの断片も候補にする。ただし、質問のロケールの見せてよい翻訳がある
///   記事の断片は除く（[`access::untranslated`]）。断片の `locale` で、どちらの言語かが分かる。
/// - `masked` はマスクした質問文。埋め込みは検索語と同じく「埋め込みの作成」の予算から出す（尽きていればキーワード検索だけ）。
#[allow(clippy::too_many_arguments)]
pub async fn fragments(
    services: &Services<'_>,
    tx: &mut Transaction<'static, Postgres>,
    workspace_id: Uuid,
    manifest: &Manifest,
    masked: &str,
    locale: &str,
    k: usize,
    min_similarity: f64,
) -> Result<Vec<Fragment>> {
    if k == 0 || masked.trim().is_empty() {
        return Ok(Vec::new());
    }
    let serving = super::switch::serving(services.embedding, tx).await?;
    let vector_setup = match (serving, services.egress) {
        (Some(c), Some(e)) => super::vector_schema(&mut **tx).await?.map(|vs| (c, e, vs)),
        _ => None,
    };
    let keyword_schema = super::keyword_schema(tx).await?;
    let (types, keys) = access::current_keys(manifest);
    let limit = i64::try_from(k)
        .unwrap_or(i64::MAX)
        .saturating_mul(4)
        .max(20);
    // 質問のロケールと、既定のロケールへのフォールバック（そのロケールの翻訳がない記事だけ。09-qa-chatbot.md）。
    // ロケールで点数を変えない（同じ並びに混ぜて RRF で順位を付ける）
    let mut targets = vec![Target {
        types: &types,
        keys: &keys,
        locale,
        untranslated: None,
        limit,
        per_entry: false,
    }];
    if manifest.default_locale != locale {
        targets.push(Target {
            types: &types,
            keys: &keys,
            locale: &manifest.default_locale,
            untranslated: Some(locale),
            limit,
            per_entry: false,
        });
    }
    let vector = match &vector_setup {
        Some((llm_config, egress, _)) => {
            query_vector(services, workspace_id, llm_config, egress, masked).await?
        }
        None => None,
    };
    let mut vector_rows = Vec::new();
    let mut keyword_rows = Vec::new();
    for target in &targets {
        if let (Some((llm_config, _, vs)), Some(vector)) = (&vector_setup, &vector) {
            vector_rows.extend(
                vector_candidates(tx, target, vs, llm_config, vector, limit)
                    .await?
                    .into_iter()
                    .filter(|r| r.5 >= min_similarity),
            );
        }
        if let Some(bs) = &keyword_schema {
            keyword_rows.extend(keyword_candidates(tx, target, bs, &terms(masked)).await?);
        }
    }
    // ロケールごとの候補を、点数の順の1つの並びにする
    for rows in [&mut vector_rows, &mut keyword_rows] {
        rows.sort_by(|a, b| b.5.total_cmp(&a.5).then(a.0.cmp(&b.0)));
    }
    Ok(fuse_chunks(vector_rows, keyword_rows, k)
        .into_iter()
        .map(|f| {
            let (entry_id, locale, anchor, breadcrumb, content, _) = f.row;
            Fragment {
                entry_id,
                locale,
                anchor,
                breadcrumb,
                content,
                score: f.score,
                vector_similarity: f.vector,
                keyword_score: f.keyword,
            }
        })
        .collect())
}

/// 混ぜた後のチャンク（RRF の点数と、それぞれの方式のもとの点数）。
struct Fused {
    score: f64,
    row: HitRow,
    vector: Option<f64>,
    keyword: Option<f64>,
}

/// RRF で2つの並びを、チャンクのまま混ぜる（同じチャンクは記事・アンカー・本文で見分ける）。
fn fuse_chunks(vector: Vec<HitRow>, keyword: Vec<HitRow>, k: usize) -> Vec<Fused> {
    type Key = (Uuid, Option<String>, String);
    let mut scores: HashMap<Key, Fused> = HashMap::new();
    let rrf = |rank: usize| 1.0 / (RRF_K + rank as f64);
    for (is_vector, rows) in [(true, vector), (false, keyword)] {
        let r = ranks(&rows);
        for (rank, row) in r.into_iter().zip(rows) {
            let key = (row.0, row.2.clone(), row.4.clone());
            let original = row.5;
            let e = scores.entry(key).or_insert(Fused {
                score: 0.0,
                row,
                vector: None,
                keyword: None,
            });
            e.score += rrf(rank);
            // 同じチャンクが2回あれば、先（点数の高いほう）の点数を残す
            let slot = if is_vector {
                &mut e.vector
            } else {
                &mut e.keyword
            };
            slot.get_or_insert(original);
        }
    }
    let mut fused: Vec<Fused> = scores.into_values().collect();
    fused.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(a.row.0.cmp(&b.row.0))
            .then(a.row.4.cmp(&b.row.4))
    });
    fused.truncate(k);
    fused
}

/// 候補を取る範囲（見せてよいチャンクの条件に渡すもの）。
struct Target<'a> {
    types: &'a [String],
    keys: &'a [String],
    locale: &'a str,
    /// 既定のロケールへのフォールバックで、このロケールの翻訳がある記事を除くか（[`access::untranslated`]）。
    untranslated: Option<&'a str>,
    /// 取る行の数。
    limit: i64,
    /// 記事ごとに、いちばん合うチャンクだけにするか（しなければ、チャンクのまま点数の順）。
    per_entry: bool,
}

/// 候補を点数の順に並べる SQL。`columns` は `c`（チャンク）から記事のID・ロケール・アンカー・パンくず・本文・点数（`score`）を
/// 選ぶ列、`from_where` は FROM から後、`limit` は取る数のパラメーターの番号。
fn ranked(columns: &str, from_where: &str, per_entry: bool, limit: usize) -> String {
    if per_entry {
        format!(
            "SELECT entry_id, locale, anchor, breadcrumb, content, score FROM (
               SELECT DISTINCT ON (c.entry_id) {columns} {from_where}
               ORDER BY c.entry_id, score DESC, c.seq
             ) best
             ORDER BY score DESC, entry_id
             LIMIT ${limit}"
        )
    } else {
        format!(
            "SELECT {columns} {from_where} ORDER BY score DESC, c.entry_id, c.seq LIMIT ${limit}"
        )
    }
}

const COLUMNS: &str = "c.entry_id, c.locale, c.anchor, c.breadcrumb, c.content";

/// 検索語の埋め込み（キャッシュになければ、予算の範囲で作る）。予算が尽きていれば None。
async fn query_vector(
    services: &Services<'_>,
    workspace_id: Uuid,
    llm_config: &LlmConfig,
    egress: &Egress,
    masked: &str,
) -> Result<Option<Vec<f32>>> {
    let dims = i32::try_from(llm_config.embedding_dimensions).unwrap_or(i32::MAX);
    let key: [u8; 32] = Sha256::new()
        .chain_update(workspace_id.as_bytes())
        .chain_update(llm_config.embedding_model.as_bytes())
        .chain_update(dims.to_be_bytes())
        .chain_update(masked.as_bytes())
        .finalize()
        .into();
    if let Some(v) = services.cache.get(&key) {
        return Ok(Some(v));
    }
    let llm = Llm::new(egress.clone(), llm_config.clone());
    match budget::embed_from(
        services.pool,
        services.config,
        &llm,
        workspace_id,
        &[masked.to_owned()],
        services.query_budget,
    )
    .await?
    {
        Embedded::Done(mut e) => {
            let Some(v) = e.vectors.pop() else {
                bail!(Unavailable("invalid_response"));
            };
            services.cache.put(key, v.clone());
            Ok(Some(v))
        }
        Embedded::OverBudget => Ok(None),
        Embedded::Failed(e) => bail!(Unavailable(e.code())),
    }
}

/// ベクトル検索の候補（チャンクのまま、点数の順に `limit` 件。点数はコサイン類似度）。
/// - 埋め込みはワークスペース・モデルごとの HNSW の索引（`super::ensure_vector_index`）で近い順に探す。索引の式と同じ形
///   （`vector(次元数)` への変換）で並べる。次元数は設定の整数。
/// - 公開状態などで絞り込んでも件数が欠けないよう、索引をたどり直す設定（`hnsw.iterative_scan = relaxed_order`）にする。
///   この設定では順番が少し前後しうるので、取った後に点数で並べ直す。
/// - モデルの条件で部分索引を使えるよう、パラメーターの値で毎回計画を立てる（`plan_cache_mode = force_custom_plan`）。
async fn vector_candidates(
    tx: &mut Transaction<'static, Postgres>,
    t: &Target<'_>,
    vs: &Ident,
    llm_config: &LlmConfig,
    vector: &[f32],
    limit: i64,
) -> Result<Vec<HitRow>> {
    let d = llm_config.embedding_dimensions;
    let dims = i32::try_from(d).unwrap_or(i32::MAX);
    for setting in [
        "SET LOCAL hnsw.iterative_scan = relaxed_order",
        "SET LOCAL plan_cache_mode = force_custom_plan",
    ] {
        sqlx::query(setting).execute(&mut **tx).await?;
    }
    let visible = access::visible_embedded(1, 2, 3, 4, 5);
    let untranslated = access::untranslated(8);
    let distance = format!(
        "(x.embedding::{vs}.vector({d})) OPERATOR({vs}.<=>) (($6::real[])::{vs}.vector({d}))"
    );
    let sql = format!(
        "SELECT {COLUMNS}, (1 - ({distance}))::float8 AS score
         FROM search_embeddings x, search_chunks c, entries e
         WHERE {visible} AND {untranslated}
         ORDER BY {distance}
         LIMIT $7"
    );
    let mut rows: Vec<HitRow> = sqlx::query_as(crate::db::trusted(sql))
        .bind(t.types)
        .bind(t.keys)
        .bind(t.locale)
        .bind(&llm_config.embedding_model)
        .bind(dims)
        .bind(vector)
        .bind(limit)
        .bind(t.untranslated)
        .fetch_all(&mut **tx)
        .await?;
    rows.sort_by(|a, b| b.5.total_cmp(&a.5).then(a.0.cmp(&b.0)));
    Ok(rows)
}

/// キーワード検索の候補（すべての語を含むチャンク。点数は pg_bigm の類似度）。
async fn keyword_candidates(
    tx: &mut Transaction<'static, Postgres>,
    t: &Target<'_>,
    bs: &Ident,
    terms: &[String],
) -> Result<Vec<HitRow>> {
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let visible = access::visible(1, 2, 3);
    let untranslated = access::untranslated(6);
    // 語ごとに LIKE を1つ（pg_bigm の索引は lower(content) に作ってある。`super::ensure_keyword_index`）
    let likes: Vec<String> = (0..terms.len())
        .map(|i| format!("lower(c.content) LIKE ('%' || lower(${}) || '%')", 7 + i))
        .collect();
    let columns =
        format!("{COLUMNS}, {bs}.bigm_similarity(lower($4), lower(c.content))::float8 AS score");
    let from_where = format!(
        "FROM search_chunks c, entries e WHERE {visible} AND {untranslated} AND {}",
        likes.join(" AND ")
    );
    let sql = ranked(&columns, &from_where, t.per_entry, 5);
    let mut q = sqlx::query_as(crate::db::trusted(sql))
        .bind(t.types)
        .bind(t.keys)
        .bind(t.locale)
        .bind(terms.join(" "))
        .bind(t.limit)
        .bind(t.untranslated);
    for term in terms {
        q = q.bind(escape_like(term));
    }
    Ok(q.fetch_all(&mut **tx).await?)
}

/// キーワード検索に使う語（空白で区切る。全角の空白も区切り。[`MAX_TERMS`] 個まで）。
fn terms(masked: &str) -> Vec<String> {
    masked
        .split(char::is_whitespace)
        .filter(|t| !t.is_empty())
        .take(MAX_TERMS)
        .map(str::to_owned)
        .collect()
}

/// LIKE の中で、語をそのままの文字として扱う（`\`・`%`・`_` を逃がす。LIKE の既定のエスケープ文字は `\`）。
fn escape_like(term: &str) -> String {
    let mut out = String::with_capacity(term.len());
    for c in term.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// 点数の順のチャンクから、記事ごとに一番上のものだけを残す（順番はそのまま）。
fn best_per_entry(rows: Vec<HitRow>) -> Vec<HitRow> {
    let mut seen = std::collections::HashSet::new();
    rows.into_iter().filter(|r| seen.insert(r.0)).collect()
}

enum Source {
    Vector(HitRow),
    Keyword(HitRow),
}

/// 点数の順の並びの、それぞれの順位（1から。同じ点数なら同じ順位にする。中身が同じ記事の点数をそろえるため）。
fn ranks(rows: &[HitRow]) -> Vec<usize> {
    let mut out = Vec::with_capacity(rows.len());
    for (i, row) in rows.iter().enumerate() {
        let rank = match (i.checked_sub(1), out.last()) {
            (Some(prev), Some(&r)) if rows[prev].5 == row.5 => r,
            _ => i + 1,
        };
        out.push(rank);
    }
    out
}

/// RRF で2つの並びを混ぜる（`1 / (k + 順位)` の和の順。同じ点なら記事のIDの順）。
fn fuse(vector: Vec<HitRow>, keyword: Vec<HitRow>, first: usize) -> Vec<(f64, Source)> {
    let mut scores: HashMap<Uuid, (f64, Option<Source>)> = HashMap::new();
    let rrf = |rank: usize| 1.0 / (RRF_K + rank as f64);
    let keyword_ranks = ranks(&keyword);
    for (rank, row) in keyword_ranks.into_iter().zip(keyword) {
        scores.insert(row.0, (rrf(rank), Some(Source::Keyword(row))));
    }
    let vector_ranks = ranks(&vector);
    for (rank, row) in vector_ranks.into_iter().zip(vector) {
        let e = scores.entry(row.0).or_insert((0.0, None));
        e.0 += rrf(rank);
        if e.1.is_none() {
            e.1 = Some(Source::Vector(row));
        }
    }
    let mut fused: Vec<(Uuid, f64, Source)> = scores
        .into_iter()
        .filter_map(|(id, (score, from))| from.map(|f| (id, score, f)))
        .collect();
    fused.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    fused.truncate(first);
    fused
        .into_iter()
        .map(|(_, score, from)| (score, from))
        .collect()
}

/// Markdown をプレーンテキストにする（印を外し、空白をまとめる）。
fn plain(markdown: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut in_fence = false;
    for line in markdown.lines() {
        let t = line.trim();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        let t = if in_fence { t } else { strip_block_marker(t) };
        if !t.is_empty() {
            words.push(if in_fence {
                t.to_owned()
            } else {
                strip_inline(t)
            });
        }
    }
    words
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Markdown をプレーンテキストにして、[`EXCERPT_CHARS`] 文字までにする。
pub fn excerpt(markdown: &str) -> String {
    let plain = plain(markdown);
    if plain.chars().count() <= EXCERPT_CHARS {
        plain
    } else {
        let mut s: String = plain.chars().take(EXCERPT_CHARS - 1).collect();
        s.push('…');
        s
    }
}

/// 大文字・小文字を区別せずに比べるための文字（1文字のまま小文字にできるものだけ小文字にする。位置をずらさないため）。
fn fold(c: char) -> char {
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(l), None) => l,
        _ => c,
    }
}

/// `haystack` の `from` 以降で、`needle` が最初に現れる位置（文字の単位）。
fn find(haystack: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (from..=haystack.len() - needle.len()).find(|&i| haystack[i..i + needle.len()] == *needle)
}

/// キーワードで当たったチャンクの抜粋（[`EXCERPT_CHARS`] 文字まで）と、その中で語と一致した箇所。
/// 最初に一致した箇所のまわりを切り出す（前に [`CONTEXT_BEFORE`] 文字を残す）。切り出した前後には `…` を付ける。
/// 一致した箇所の位置と長さは、抜粋の中の UTF-16 の単位（JS の文字列の位置と同じ）。
fn excerpt_around(markdown: &str, terms: &[String]) -> (String, Vec<Match>) {
    let text: Vec<char> = plain(markdown).chars().collect();
    let folded: Vec<char> = text.iter().copied().map(fold).collect();
    let needles: Vec<Vec<char>> = terms
        .iter()
        .map(|t| t.chars().map(fold).collect::<Vec<char>>())
        .filter(|t| !t.is_empty())
        .collect();
    let n = text.len();
    // 切り出す範囲 [start, end) と、前後の … の有無
    let (start, end) = if n <= EXCERPT_CHARS {
        (0, n)
    } else {
        let hit = needles.iter().filter_map(|t| find(&folded, t, 0)).min();
        // 末尾の近くで当たったときも、200文字を埋める（前の … の分の1文字を残す）
        let s = hit
            .map_or(0, |p| p.saturating_sub(CONTEXT_BEFORE))
            .min(n - (EXCERPT_CHARS - 1));
        if s == 0 {
            (0, EXCERPT_CHARS - 1)
        } else if s + (EXCERPT_CHARS - 1) >= n {
            (s, n)
        } else {
            (s, s + EXCERPT_CHARS - 2)
        }
    };
    let prefix = start > 0;
    let suffix = end < n;
    let mut excerpt = String::new();
    if prefix {
        excerpt.push('…');
    }
    excerpt.extend(&text[start..end]);
    if suffix {
        excerpt.push('…');
    }
    // 範囲の中の一致（重なるものはまとめる）
    let window = &folded[start..end];
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for needle in &needles {
        let mut from = 0;
        while let Some(p) = find(window, needle, from) {
            spans.push((p, p + needle.len()));
            from = p + needle.len();
        }
    }
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in spans {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    // 文字の位置を UTF-16 の位置にする（前の … は1単位）
    let offset = usize::from(prefix);
    let mut utf16 = Vec::with_capacity(window.len() + 1);
    let mut acc = offset;
    utf16.push(acc);
    for c in &text[start..end] {
        acc += c.len_utf16();
        utf16.push(acc);
    }
    let matches = merged
        .into_iter()
        .map(|(s, e)| Match {
            start: utf16[s],
            length: utf16[e] - utf16[s],
        })
        .collect();
    (excerpt, matches)
}

/// 行の頭の印（見出し・引用・箇条書き・番号付きの箇条書き）を外す。
fn strip_block_marker(line: &str) -> &str {
    let mut t = line;
    loop {
        let before = t;
        t = t.trim_start_matches('#').trim_start();
        t = t
            .strip_prefix("> ")
            .or_else(|| t.strip_prefix('>'))
            .unwrap_or(t)
            .trim_start();
        for m in ["- ", "* ", "+ "] {
            t = t.strip_prefix(m).unwrap_or(t);
        }
        let digits = t.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && t[digits..].starts_with(". ") {
            t = &t[digits + 2..];
        }
        if t == before {
            return t;
        }
    }
}

/// 行の中の印（画像・リンク・自動リンク・強調・コード・エスケープ）を外す。
fn strip_inline(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // ![代替テキスト](参照先) / [文字](URL) → 代替テキスト / 文字
        if (c == '[' || (c == '!' && chars.get(i + 1) == Some(&'[')))
            && let Some((label, end)) = bracket_link(&chars, if c == '!' { i + 1 } else { i })
        {
            out.push_str(&strip_inline(&label));
            i = end;
            continue;
        }
        // <https://...> （埋め込み・自動リンク）は出さない
        if c == '<'
            && let Some(close) = chars[i..].iter().position(|x| *x == '>')
        {
            let inner: String = chars[i + 1..i + close].iter().collect();
            if inner.contains("://") {
                i += close + 1;
                continue;
            }
        }
        match c {
            '\\' if chars.get(i + 1).is_some_and(|n| n.is_ascii_punctuation()) => {
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            '*' | '`' => {}
            '_' if chars.get(i + 1) == Some(&'_')
                || i == 0
                || chars.get(i - 1).is_some_and(|p| p.is_whitespace()) => {}
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

/// `[文字](参照先)` の文字と、その次の位置。
fn bracket_link(chars: &[char], open: usize) -> Option<(String, usize)> {
    let close = open + chars[open..].iter().position(|c| *c == ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = close + 1 + chars[close + 1..].iter().position(|c| *c == ')')?;
    Some((chars[open + 1..close].iter().collect(), end + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excerpt_is_plain_text() {
        let md = "見出し\n\n**太字**と`コード`と[リンク](https://example.com/a)。\n\n- 箇条書き\n1. 番号\n\n> 引用\n\n![図の説明](yomi-media:abc)\n\n<https://www.youtube.com/watch?v=x>\n\n```\nlet x = 1;\n```\n\\*記号\\*";
        assert_eq!(
            excerpt(md),
            "見出し 太字とコードとリンク。 箇条書き 番号 引用 図の説明 let x = 1; *記号*"
        );
        assert!(!excerpt(md).contains("https://"));
        assert_eq!(excerpt("snake_case_name"), "snake_case_name");
    }

    #[test]
    fn excerpt_is_at_most_200_chars() {
        let e = excerpt(&"あ".repeat(500));
        assert_eq!(e.chars().count(), EXCERPT_CHARS);
        assert!(e.ends_with('…'));
        assert_eq!(excerpt("短い"), "短い");
    }

    #[test]
    fn terms_are_split_by_whitespace_and_limited() {
        assert_eq!(
            terms("全文検索　PostgreSQL  索引"),
            vec!["全文検索", "PostgreSQL", "索引"]
        );
        assert_eq!(terms("a b c d e f g h i j").len(), MAX_TERMS);
        assert_eq!(terms("猫"), vec!["猫"], "1文字の語も使う");
        assert!(terms(" 　 ").is_empty());
    }

    #[test]
    fn like_wildcards_are_escaped() {
        assert_eq!(escape_like("100%_a\\b"), "100\\%\\_a\\\\b");
        assert_eq!(escape_like("検索"), "検索");
    }

    #[test]
    fn modes_round_trip() {
        for m in Mode::ALL {
            assert_eq!(Mode::from_name(m.name()), Some(m));
        }
        assert_eq!(Mode::from_name("hybrid"), None);
    }

    fn row(id: u128, score: f64) -> HitRow {
        (
            Uuid::from_u128(id),
            "ja".into(),
            None,
            Vec::new(),
            format!("本文{id}"),
            score,
        )
    }

    #[test]
    fn fuse_adds_reciprocal_ranks() {
        // 1 は両方の1位、2 はベクトル検索だけの2位、3 はキーワード検索だけの2位
        let fused = fuse(
            vec![row(1, 0.9), row(2, 0.8)],
            vec![row(1, 0.5), row(3, 0.4)],
            10,
        );
        let ids: Vec<u128> = fused
            .iter()
            .map(|(_, f)| match f {
                Source::Vector(r) | Source::Keyword(r) => r.0.as_u128(),
            })
            .collect();
        assert_eq!(ids, vec![1, 2, 3], "同じ点なら記事のIDの順");
        assert!((fused[0].0 - 2.0 / 61.0).abs() < 1e-12);
        assert!((fused[1].0 - 1.0 / 62.0).abs() < 1e-12);
        // 両方で当たった記事は、キーワード検索のチャンクを見せる
        assert!(matches!(fused[0].1, Source::Keyword(_)));
        assert!(matches!(fused[1].1, Source::Vector(_)));
        assert_eq!(fuse(vec![row(1, 0.9), row(2, 0.8)], Vec::new(), 1).len(), 1);
        // 同じ点数の記事は、同じ順位（同じ RRF の点）になる
        let tied = fuse(vec![row(5, 0.7), row(4, 0.7), row(6, 0.1)], Vec::new(), 10);
        assert_eq!(tied[0].0, tied[1].0);
        assert!((tied[2].0 - 1.0 / 63.0).abs() < 1e-12);
    }

    #[test]
    fn best_per_entry_keeps_the_first_chunk() {
        let rows = vec![row(1, 0.9), row(2, 0.8), row(1, 0.7)];
        let best = best_per_entry(rows);
        assert_eq!(best.len(), 2);
        assert_eq!(best[0].5, 0.9);
    }

    #[test]
    fn excerpt_around_marks_matches_in_utf16() {
        let (e, m) = excerpt_around(
            "**PostgreSQL** の全文検索と postgresql の索引",
            &["postgresql".into(), "索引".into()],
        );
        assert_eq!(e, "PostgreSQL の全文検索と postgresql の索引");
        let pick = |m: &Match| {
            e.encode_utf16()
                .skip(m.start)
                .take(m.length)
                .collect::<Vec<u16>>()
        };
        let picked: Vec<String> = m
            .iter()
            .map(|x| String::from_utf16(&pick(x)).unwrap())
            .collect();
        assert_eq!(picked, vec!["PostgreSQL", "postgresql", "索引"]);
        // UTF-16 で2単位の文字の後ろでも、位置がずれない
        let (e, m) = excerpt_around("𠮷野家の牛丼", &["牛丼".into()]);
        assert_eq!(
            m,
            vec![Match {
                start: 5,
                length: 2
            }]
        );
        assert_eq!(
            String::from_utf16(&e.encode_utf16().skip(5).take(2).collect::<Vec<_>>()).unwrap(),
            "牛丼"
        );
    }

    #[test]
    fn excerpt_around_centers_on_the_first_match() {
        let md = format!("{}目印{}", "あ".repeat(300), "い".repeat(300));
        let (e, m) = excerpt_around(&md, &["目印".into()]);
        assert_eq!(e.chars().count(), EXCERPT_CHARS);
        assert!(e.starts_with('…') && e.ends_with('…'));
        assert_eq!(
            m,
            vec![Match {
                start: 1 + CONTEXT_BEFORE,
                length: 2
            }]
        );
        // 末尾の近くで当たったときは、末尾まで入れて200文字にする
        let md = format!("{}目印", "あ".repeat(300));
        let (e, m) = excerpt_around(&md, &["目印".into()]);
        assert_eq!(e.chars().count(), EXCERPT_CHARS);
        assert!(e.starts_with('…') && e.ends_with("目印"));
        assert_eq!(m[0].start, EXCERPT_CHARS - 2);
        // 当たらなければ（本文の印の中だけで当たったなど）、先頭から切り出し、一致は空
        let (e, m) = excerpt_around(&"う".repeat(300), &["目印".into()]);
        assert_eq!(e, excerpt(&"う".repeat(300)));
        assert!(m.is_empty());
    }

    #[test]
    fn cache_keeps_recent_entries_only() {
        let cache = Cache::default();
        cache.put([1; 32], vec![1.0]);
        assert_eq!(cache.get(&[1; 32]), Some(vec![1.0]));
        assert_eq!(cache.get(&[2; 32]), None);
        for i in 0..CACHE_MAX + 5 {
            let mut k = [0u8; 32];
            k[..8].copy_from_slice(&(i as u64).to_be_bytes());
            k[31] = 9;
            cache.put(k, vec![0.0]);
        }
        assert!(cache.map.lock().unwrap().len() <= CACHE_MAX);
    }
}
