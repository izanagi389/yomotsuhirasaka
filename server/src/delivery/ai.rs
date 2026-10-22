//! AI向けの素材（07-ai-delivery.md）: 記事のMarkdown版、目録、読了時間、学習への利用を拒否した記事のURL。
//!
//! - 素材にするのは、コンテンツタイプに `aiMarkdown` があり、記事の「外部AI向けの素材に含める」がONの公開中の記事だけ。
//!   スイッチは返すたびにいまの値と照合し、保存済みの素材を信用しない（原則3、INV-AIX-01）。関係の相手も同じ条件で絞る。
//! - Markdown版に載せるフィールドは `aiMarkdown` に挙げたものだけ（INV-AIX-02）。`devNote` はマニフェストから読まない（INV-AIX-03）。
//! - 本文は、公開のときに変換器で作って版に保存したMarkdown（`versions.rendered`）を使う。画像はその時点のメディアのURLにする。
//! - 冒頭のメタデータ（front matter）の値は、JSONの書き方で書く（JSONはYAMLとしても読める。エスケープを自分で書かない）。

use std::collections::BTreeMap;

use anyhow::Result;
use serde::Serialize;
use serde_json::{Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::data::{self, Entry};
use crate::content::{corrections, freshness};
use crate::schema::manifest::{ContentType, FieldType, Manifest};

/// 目録の1回の件数の上限（既定も同じ）。
pub const CATALOG_LIMIT: i64 = 1000;
/// 学習への利用を拒否した記事のURLの件数の上限。
pub const OPT_OUTS_LIMIT: i64 = 10_000;

const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;

/// 読了時間（分）。変換器が作ったMarkdownから数える（Q64）。日本語・中国語・韓国語の文字は1分に500文字、それ以外は1分に200語。
/// 切り上げで、最低1分。
pub fn reading_time(ct: &ContentType, rendered: &Map<String, Value>) -> i32 {
    let mut cjk = 0u64;
    let mut words = 0u64;
    for f in ct.fields.iter().filter(|f| f.kind == FieldType::Richtext) {
        let Some(md) = rendered
            .get(&f.name)
            .and_then(|r| r.get("markdown"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let (c, w) = count_text(&strip_markdown(md));
        cjk += c;
        words += w;
    }
    let minutes = (cjk as f64 / 500.0 + words as f64 / 200.0).ceil() as i32;
    minutes.max(1)
}

/// 読む文字だけを残す（リンク先・画像・見出しのアンカーID・エスケープの `\` を除く）。
fn strip_markdown(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let chars: Vec<char> = md.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // `](...)` のリンク先と、`{#...}` のアンカーID
        if (c == ']' && chars.get(i + 1) == Some(&'('))
            || (c == '{' && chars.get(i + 1) == Some(&'#'))
        {
            let close = if c == ']' { ')' } else { '}' };
            if let Some(p) = chars[i + 1..].iter().position(|&x| x == close) {
                i += p + 2;
                out.push(' ');
                continue;
            }
        }
        if c == '\\' {
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF   // ひらがな・カタカナ
        | 0x3400..=0x4DBF // CJK統合漢字拡張A
        | 0x4E00..=0x9FFF // CJK統合漢字
        | 0xF900..=0xFAFF // CJK互換漢字
        | 0xAC00..=0xD7AF // ハングル
        | 0x20000..=0x2FFFF)
}

/// (CJKの文字数, それ以外の語数)。語は、文字か数字を含むひと続き。
fn count_text(text: &str) -> (u64, u64) {
    let mut cjk = 0;
    let mut words = 0;
    let mut in_word = false;
    for c in text.chars() {
        if is_cjk(c) {
            cjk += 1;
            in_word = false;
        } else if c.is_alphanumeric() {
            if !in_word {
                words += 1;
                in_word = true;
            }
        } else if c.is_whitespace() {
            in_word = false;
        }
    }
    (cjk, words)
}

/// Markdownで意味を持つ文字をエスケープする（変換器の `escapeMarkdown` と同じ規則。packages/converter/src/index.ts）。
pub fn escape_markdown(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for line in s.split('\n') {
        if !out.is_empty() {
            out.push('\n');
        }
        let trimmed = line.trim_start();
        let indent = &line[..line.len() - trimmed.len()];
        out.push_str(indent);
        let mut rest = trimmed;
        // 行頭の `-`・`=`（リストと見出しの下線）と、`1.`（番号付きリスト）
        if rest.starts_with(['-', '=']) {
            out.push('\\');
        } else {
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            if digits > 0 && rest[digits..].starts_with('.') {
                out.push_str(&rest[..digits]);
                out.push_str("\\.");
                rest = &rest[digits + 1..];
            }
        }
        for c in rest.chars() {
            if "\\`*_{}[]()<>#+!|~".contains(c) {
                out.push('\\');
            }
            out.push(c);
        }
    }
    out
}

/// 冒頭のメタデータの値（JSON）。U+2028・U+2029 は、YAMLとして読むときに改行として扱われないようエスケープする。
fn front_value(v: &Value) -> String {
    serde_json::to_string(v)
        .unwrap_or_else(|_| "null".into())
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

fn text_of<'a>(fields: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    fields
        .get(name)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

/// 参照フィールドの値（翻訳グループのID）。
fn groups_of(fields: &Map<String, Value>, name: &str) -> Vec<Uuid> {
    match fields.get(name) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .filter_map(|s| Uuid::parse_str(s).ok())
            .collect(),
        Some(Value::String(s)) => Uuid::parse_str(s).ok().into_iter().collect(),
        _ => Vec::new(),
    }
}

/// 記事がAI向けの素材になるか（いまのスイッチと、コンテンツタイプの指定）。
pub fn is_material(manifest: &Manifest, e: &Entry) -> bool {
    e.include_in_ai_materials
        && manifest
            .content_type(&e.content_type)
            .is_some_and(|ct| ct.ai_markdown.is_some())
}

/// 記事のMarkdown版。素材にならない記事では None。
pub async fn markdown(
    conn: &mut PgConnection,
    manifest: &Manifest,
    media_base: &str,
    workspace_id: Uuid,
    e: &Entry,
) -> Result<Option<String>> {
    if !is_material(manifest, e) {
        return Ok(None);
    }
    let Some(ct) = manifest.content_type(&e.content_type) else {
        return Ok(None);
    };
    let Some(ai) = &ct.ai_markdown else {
        return Ok(None);
    };
    let title = text_of(&e.fields, &ai.title).unwrap_or_default();

    // 冒頭のメタデータ
    let mut front: Vec<(String, Value)> = vec![
        ("title".into(), Value::from(title)),
        ("url".into(), Value::from(e.url.clone())),
        ("locale".into(), Value::from(e.locale.clone())),
        ("updated".into(), Value::from(e.published_at.clone())),
    ];
    let default_days = ct.review.as_ref().map(|r| r.interval_days);
    if let Some(at) = freshness::get(conn, default_days, e.id)
        .await?
        .and_then(|f| f.last_verified_at)
    {
        front.push(("lastVerified".into(), Value::from(at)));
    }
    front.push((
        "readingTime".into(),
        Value::from(reading_time(ct, &e.rendered)),
    ));
    if e.opt_out_of_ai_training {
        front.push(("aiTraining".into(), Value::from("disallowed")));
    }
    for name in &ai.meta {
        if let Some(v) = text_of(&e.fields, name) {
            front.push((name.clone(), Value::from(v)));
        }
    }
    // 関係: 種類（フィールドの名前）→ 相手のURL。相手も素材になる記事だけ（INV-AIX-01）
    let mut relations = Map::new();
    for name in &ai.relations {
        let Some(target_ct) = ct
            .fields
            .iter()
            .find(|f| &f.name == name)
            .and_then(|f| f.reference_to.as_deref())
        else {
            continue;
        };
        let groups = groups_of(&e.fields, name);
        let urls: Vec<Value> = data::resolve_groups(conn, target_ct, &e.locale, &groups)
            .await?
            .into_iter()
            .filter(|t| is_material(manifest, t) && t.id != e.id)
            .map(|t| Value::from(t.url))
            .collect();
        if !urls.is_empty() {
            relations.insert(name.clone(), Value::Array(urls));
        }
    }
    if !relations.is_empty() {
        front.push(("relations".into(), Value::Object(relations)));
    }
    let corrections: Vec<Value> = corrections::entry_corrections(conn, e.id, &e.locale)
        .await?
        .into_iter()
        .map(|c| {
            serde_json::json!({ "correctedAt": c.corrected_at, "description": c.description, "sections": c.sections })
        })
        .collect();
    if !corrections.is_empty() {
        front.push(("corrections".into(), Value::Array(corrections)));
    }
    let deleted = corrections::deleted_anchors(conn, e.id, &e.locale).await?;
    if !deleted.is_empty() {
        front.push(("deletedAnchors".into(), serde_json::to_value(deleted)?));
    }

    let mut out = String::from("---\n");
    for (k, v) in &front {
        out.push_str(&format!("{k}: {}\n", front_value(v)));
    }
    out.push_str("---\n\n# ");
    out.push_str(&escape_markdown(title));
    out.push('\n');
    // 本文: 挙げたフィールドを順に
    for name in &ai.fields {
        let kind = ct.fields.iter().find(|f| &f.name == name).map(|f| f.kind);
        let part = match kind {
            Some(FieldType::Richtext) => {
                match e
                    .rendered
                    .get(name)
                    .and_then(|r| r.get("markdown"))
                    .and_then(Value::as_str)
                {
                    Some(md) => {
                        Some(data::resolve_media_refs(conn, media_base, workspace_id, md).await?)
                    }
                    None => None,
                }
            }
            Some(FieldType::Text) => text_of(&e.fields, name).map(escape_markdown),
            _ => None,
        };
        if let Some(part) = part.filter(|p| !p.trim().is_empty()) {
            out.push('\n');
            out.push_str(part.trim_end());
            out.push('\n');
        }
    }
    Ok(Some(out))
}

/// 目録の1件。
#[derive(Debug, Clone, Serialize)]
pub struct CatalogItem {
    pub entry_id: Uuid,
    pub content_type: String,
    pub url: String,
    pub title: Option<String>,
    pub locale: String,
    pub updated_at: String,
}

/// 目録の行: (記事のID, コンテンツタイプ, URL, 見出し, 更新日時)
type CatalogRow = (Uuid, String, Option<String>, Option<String>, String);

/// 目録（ロケールの素材になる記事。更新の新しい順）。
pub async fn catalog(
    conn: &mut PgConnection,
    manifest: &Manifest,
    locale: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<CatalogItem>> {
    // 素材になるコンテンツタイプと、その見出しのフィールド
    let types: BTreeMap<&str, &str> = manifest
        .content_types
        .iter()
        .filter_map(|ct| {
            ct.ai_markdown
                .as_ref()
                .map(|ai| (ct.name.as_str(), ai.title.as_str()))
        })
        .collect();
    if types.is_empty() || limit <= 0 {
        return Ok(Vec::new());
    }
    let names: Vec<&str> = types.keys().copied().collect();
    let titles: Vec<&str> = types.values().copied().collect();
    let rows: Vec<CatalogRow> = sqlx::query_as(crate::sql!(
        "SELECT e.id, e.content_type, e.url, v.fields ->> t.title_field,
                to_char(v.published_at AT TIME ZONE 'UTC', {RFC3339})
         FROM entries e
         JOIN versions v ON v.id = e.published_version_id
         JOIN unnest($1::text[], $2::text[]) AS t(content_type, title_field) ON t.content_type = e.content_type
         WHERE e.deleted_at IS NULL AND v.fields IS NOT NULL AND e.locale = $3 AND e.include_in_ai_materials
         ORDER BY v.published_at DESC, e.id
         LIMIT $4 OFFSET $5"
    ))
    .bind(&names)
    .bind(&titles)
    .bind(locale)
    .bind(limit)
    .bind(offset)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(entry_id, content_type, url, title, updated_at)| CatalogItem {
                entry_id,
                content_type,
                url: url.unwrap_or_default(),
                title,
                locale: locale.to_owned(),
                updated_at,
            },
        )
        .collect())
}

/// 学習への利用を拒否した公開中の記事のURL（すべてのロケール。素材のスイッチとは別。連携部品が robots.txt に書く）。
pub async fn training_opt_outs(conn: &mut PgConnection) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT e.url FROM entries e
         WHERE e.published_version_id IS NOT NULL AND e.deleted_at IS NULL AND e.opt_out_of_ai_training
               AND e.url IS NOT NULL
         ORDER BY e.url
         LIMIT $1",
    )
    .bind(OPT_OUTS_LIMIT)
    .fetch_all(conn)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_time_counts_cjk_chars_and_words() {
        assert_eq!(count_text("日本語の文章です"), (8, 0));
        assert_eq!(count_text("hello world, 2026 年"), (1, 3));
        let stripped = strip_markdown(
            "## 見出し {#h-1}\n\n[リンク](https://example.com/very/long/path) \\*x\\*",
        );
        assert!(!stripped.contains("example.com"));
        assert!(!stripped.contains("h-1"));
        assert!(!stripped.contains('\\'));
    }

    #[test]
    fn markdown_is_escaped_like_the_converter() {
        assert_eq!(
            escape_markdown("a*b_c [x](y) <b>"),
            "a\\*b\\_c \\[x\\]\\(y\\) \\<b\\>"
        );
        assert_eq!(escape_markdown("# no heading"), "\\# no heading");
        assert_eq!(escape_markdown("- item\n1. one"), "\\- item\n1\\. one");
    }

    #[test]
    fn front_matter_values_are_json() {
        assert_eq!(front_value(&Value::from("a\"b\nc")), "\"a\\\"b\\nc\"");
        assert_eq!(front_value(&Value::from("x\u{2028}y")), "\"x\\u2028y\"");
    }
}
