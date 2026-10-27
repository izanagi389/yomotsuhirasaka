//! サイト内検索の実行（08-search-and-related.md、M3）。配信APIの `search` から呼ぶ。
//!
//! - 検索語は、外部に送る前にマスクする（`crate::mask`。Q169）。長すぎる検索語は切り詰める。
//! - 検索語の埋め込みは「埋め込みの作成」の予算から出す（`budget::embed`）。尽きていれば、ベクトル検索は予算切れを示して結果を返さない。
//! - 検索語の埋め込みは、メモリの中に10分だけ持つ。キーは（ワークスペース、モデル、マスクした検索語）の SHA-256 で、
//!   検索語そのものは持たない（原則7）。持つ数には上限がある。
//! - 見せてよいチャンクは、共通の層（`access::visible`）の条件で、検索のたびに今の状態と照合して選ぶ。
//! - 1つの記事からは、いちばん似たチャンクを1件だけ返す。類似度の足切りはしない（サイトに任せる）。
//! - 抜粋は、チャンクの本文（検索の対象のフィールドから作ったもの）をプレーンテキストにして200文字まで。HTMLは返さない。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use sha2::{Digest as _, Sha256};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::access;
use crate::budget::{self, Embedded};
use crate::config::Config;
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

#[derive(Debug, Clone)]
pub struct Hit {
    pub entry_id: Uuid,
    pub locale: String,
    pub anchor: Option<String>,
    pub breadcrumb: Vec<String>,
    pub excerpt: String,
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
    pub egress: Option<&'a Egress>,
    pub cache: &'a Cache,
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
) -> Result<Outcome> {
    // キーワード検索（日本語の拡張）は、作業5で入れる
    let keyword = Availability::Disabled;
    let disabled = Ok(Outcome {
        hits: Vec::new(),
        vector: Availability::Disabled,
        keyword,
    });
    let (Some(llm_config), Some(egress)) = (&services.config.llm, services.egress) else {
        return disabled;
    };
    let Some(vs) = super::vector_schema(&mut **tx).await? else {
        return disabled;
    };
    let first = first.clamp(0, MAX_FIRST);
    let text: String = text.trim().chars().take(MAX_QUERY_CHARS).collect();
    if first == 0 || text.is_empty() {
        return Ok(Outcome {
            hits: Vec::new(),
            vector: Availability::Available,
            keyword,
        });
    }
    let masked = crate::mask::mask(&text);
    let model = &llm_config.embedding_model;
    let dims = i32::try_from(llm_config.embedding_dimensions).unwrap_or(i32::MAX);
    let key: [u8; 32] = Sha256::new()
        .chain_update(workspace_id.as_bytes())
        .chain_update(model.as_bytes())
        .chain_update(dims.to_be_bytes())
        .chain_update(masked.as_bytes())
        .finalize()
        .into();
    let vector = match services.cache.get(&key) {
        Some(v) => v,
        None => {
            let llm = Llm::new(egress.clone(), llm_config.clone());
            match budget::embed(
                services.pool,
                services.config,
                &llm,
                workspace_id,
                &[masked],
            )
            .await?
            {
                Embedded::Done(mut e) => {
                    let Some(v) = e.vectors.pop() else {
                        bail!(Unavailable("invalid_response"));
                    };
                    services.cache.put(key, v.clone());
                    v
                }
                Embedded::OverBudget => {
                    return Ok(Outcome {
                        hits: Vec::new(),
                        vector: Availability::BudgetExhausted,
                        keyword,
                    });
                }
                Embedded::Failed(e) => bail!(Unavailable(e.code())),
            }
        }
    };

    let (types, keys) = access::current_keys(manifest);
    let visible = access::visible(1, 2, 3, 4, 5);
    let rows: Vec<HitRow> = sqlx::query_as(crate::sql!(
        "SELECT entry_id, locale, anchor, breadcrumb, content, score FROM (
           SELECT DISTINCT ON (c.entry_id) c.entry_id, c.locale, c.anchor, c.breadcrumb, c.content,
                  1 - ((c.embedding::{vs}.vector) OPERATOR({vs}.<=>) ($6::real[])::{vs}.vector) AS score
           FROM search_chunks c, entries e
           WHERE {visible}
           ORDER BY c.entry_id, score DESC
         ) best
         ORDER BY score DESC, entry_id
         LIMIT $7"
    ))
    .bind(&types)
    .bind(&keys)
    .bind(locale)
    .bind(model)
    .bind(dims)
    .bind(&vector)
    .bind(first)
    .fetch_all(&mut **tx)
    .await?;
    Ok(Outcome {
        hits: rows
            .into_iter()
            .map(
                |(entry_id, locale, anchor, breadcrumb, content, score)| Hit {
                    entry_id,
                    locale,
                    anchor,
                    breadcrumb,
                    excerpt: excerpt(&content),
                    score,
                },
            )
            .collect(),
        vector: Availability::Available,
        keyword,
    })
}

/// 検索の結果の行（記事のID・ロケール・アンカー・パンくず・本文・類似度）。
type HitRow = (Uuid, String, Option<String>, Vec<String>, String, f64);

/// Markdown をプレーンテキストにして、[`EXCERPT_CHARS`] 文字までにする。
pub fn excerpt(markdown: &str) -> String {
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
    let plain = words
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if plain.chars().count() <= EXCERPT_CHARS {
        plain
    } else {
        let mut s: String = plain.chars().take(EXCERPT_CHARS - 1).collect();
        s.push('…');
        s
    }
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
        let md = "見出し\n\n**太字**と`コード`と[リンク](https://example.com/a)。\n\n- 箇条書き\n1. 番号\n\n> 引用\n\n![図の説明](cms-media:abc)\n\n<https://www.youtube.com/watch?v=x>\n\n```\nlet x = 1;\n```\n\\*記号\\*";
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
