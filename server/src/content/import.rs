//! 既存のブログからの取り込み（03-content-and-editing.md「既存ブログからの取り込み」、Q74, Q161, Q163, Q165, Q187）。
//!
//! 取り込みのCLIが、Markdownを変換した記事をまとめて送る。サーバーはそれを1つのトランザクションで
//! 作成・公開し、元のURLから新しいURLへの対応をURLの変更履歴に残し、翻訳グループと元の記事を記録する。
//! どれか1つでも受け付けられなければ、何も書かない（INV-IMP-01）。
//!
//! 翻訳グループ（Q161, Q163, Q165）:
//! - `translationKey` が同じ記事を1つのグループにまとめる。指定がなければ記事ごとに単独のグループ。
//! - 元の記事は、`translationOf`（元の記事のスラッグ）で指定したもの。指定がなければ、既定のロケールの記事。
//!   どちらもなければ「元なし」。元の記事以外には、元の記事と、取り込んだ時点の元の記事の版を記録する。

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::content::entries;
use crate::content::fields::{self, Column};
use crate::content::publishing::{self, Refused};
use crate::schema::manifest::{ContentType, FieldType, Manifest};

/// 1回に取り込める記事の数。
pub const MAX_ENTRIES: usize = 2000;
const MAX_URL_CHARS: usize = 2048;
const MAX_KEY_CHARS: usize = 200;

/// 取り込む1件（CLIが変換したもの）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Item {
    /// エラーの表示に使う名前（元のファイル名など）。保存しない。
    pub key: String,
    pub locale: String,
    pub fields: Map<String, Value>,
    /// 元のサイトでのURL。
    pub original_url: String,
    pub translation_key: Option<String>,
    /// 元の記事のスラッグ（同じ翻訳グループの中）。
    pub translation_of: Option<String>,
}

/// 取り込んだ1件。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Imported {
    pub key: String,
    pub id: Uuid,
    pub url: String,
    pub version_id: Uuid,
}

/// 書く前の検査を通った計画。
#[derive(Debug)]
pub struct Plan {
    entries: Vec<Planned>,
}

#[derive(Debug)]
struct Planned {
    key: String,
    id: Uuid,
    locale: String,
    group: Uuid,
    /// 元の記事の、このバッチの中の番号（自分が元・元なしなら None）。
    source: Option<usize>,
    original_url: String,
    columns: Vec<(String, Column)>,
}

fn is_text(s: &str, max: usize) -> bool {
    !s.is_empty() && s.chars().count() <= max && !s.chars().any(char::is_control)
}

fn slug_of<'a>(ct: &ContentType, fields: &'a Map<String, Value>) -> Option<&'a str> {
    let f = ct.fields.iter().find(|f| f.kind == FieldType::Slug)?;
    fields.get(&f.name)?.as_str()
}

/// 書く前の検査（スキーマ、URL、翻訳グループ）。問題はすべて集めて返す。
pub fn plan(manifest: &Manifest, ct: &ContentType, items: &[Item]) -> Result<Plan, Vec<String>> {
    let mut problems = Vec::new();
    if items.is_empty() || items.len() > MAX_ENTRIES {
        return Err(vec![format!("entries: send 1 to {MAX_ENTRIES} entries")]);
    }
    let mut seen_urls: HashMap<&str, &str> = HashMap::new();
    let mut converted = Vec::with_capacity(items.len());
    for item in items {
        let k = &item.key;
        if !is_text(k, MAX_KEY_CHARS) {
            problems.push("key: must be 1 to 200 characters without control characters".to_owned());
            converted.push(None);
            continue;
        }
        if !manifest.locales.contains(&item.locale) {
            problems.push(format!("{k}: locale: not a locale of this workspace"));
        }
        match url::Url::parse(&item.original_url) {
            Ok(u)
                if matches!(u.scheme(), "http" | "https")
                    && item.original_url.len() <= MAX_URL_CHARS => {}
            _ => problems.push(format!("{k}: originalUrl: must be an absolute http(s) URL")),
        }
        if let Some(other) = seen_urls.insert(&item.original_url, k) {
            problems.push(format!("{k}: originalUrl: also used by {other}"));
        }
        for (name, v) in [
            ("translationKey", &item.translation_key),
            ("translationOf", &item.translation_of),
        ] {
            if v.as_deref().is_some_and(|v| !is_text(v, MAX_KEY_CHARS)) {
                problems.push(format!("{k}: {name}: must be 1 to 200 characters"));
            }
        }
        if item.translation_of.is_some() && item.translation_key.is_none() {
            problems.push(format!("{k}: translationOf: needs translationKey"));
        }
        // 参照は取り込みの後に編集画面で付ける（取り込みの時点では、参照先の記事のIDがない）
        for name in item.fields.keys() {
            if ct
                .field(name)
                .is_some_and(|f| f.kind == FieldType::Reference)
            {
                problems.push(format!(
                    "{k}: fields.{name}: reference fields cannot be imported"
                ));
            }
        }
        match fields::convert_all(manifest, ct, &item.fields) {
            Ok(columns) => converted.push(Some(columns)),
            Err(p) => {
                problems.extend(p.into_iter().map(|p| format!("{k}: {p}")));
                converted.push(None);
            }
        }
        if slug_of(ct, &item.fields).is_none_or(str::is_empty) {
            problems.push(format!("{k}: a slug is required to publish"));
        }
    }

    // 翻訳グループ: translationKey ごと（なければ1件ずつ）
    let mut groups: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, item) in items.iter().enumerate() {
        if let Some(key) = item.translation_key.as_deref() {
            groups.entry(key).or_default().push(i);
        }
    }
    let mut group_of: Vec<Uuid> = (0..items.len()).map(|_| Uuid::new_v4()).collect();
    let mut source_of: Vec<Option<usize>> = vec![None; items.len()];
    for (key, members) in &groups {
        let group = Uuid::new_v4();
        let mut locales: HashMap<&str, usize> = HashMap::new();
        for &i in members {
            group_of[i] = group;
            if let Some(&j) = locales.get(items[i].locale.as_str()) {
                problems.push(format!(
                    "{}: translationKey {key}: locale {} is also used by {}",
                    items[i].key, items[i].locale, items[j].key
                ));
            }
            locales.insert(&items[i].locale, i);
        }
        // 元の記事: translationOf の指定（グループの中で1つに揃っていること）、なければ既定のロケール
        let named: Vec<&str> = members
            .iter()
            .filter_map(|&i| items[i].translation_of.as_deref())
            .collect();
        let source = if let Some(first) = named.first() {
            if named.iter().any(|n| n != first) {
                problems.push(format!(
                    "translationKey {key}: translationOf must name the same entry"
                ));
                None
            } else {
                let found: Vec<usize> = members
                    .iter()
                    .copied()
                    .filter(|&i| slug_of(ct, &items[i].fields) == Some(*first))
                    .collect();
                match found.as_slice() {
                    [i] if items[*i].translation_of.is_none() => Some(*i),
                    [i] => {
                        problems.push(format!(
                            "{}: the source entry must not have translationOf",
                            items[*i].key
                        ));
                        None
                    }
                    [] => {
                        problems.push(format!(
                            "translationKey {key}: translationOf {first} is not in this group"
                        ));
                        None
                    }
                    _ => {
                        problems.push(format!(
                            "translationKey {key}: translationOf {first} matches several entries"
                        ));
                        None
                    }
                }
            }
        } else {
            members
                .iter()
                .copied()
                .find(|&i| items[i].locale == manifest.default_locale)
        };
        if let Some(s) = source {
            for &i in members {
                if i != s {
                    source_of[i] = Some(s);
                }
            }
        }
    }

    if !problems.is_empty() {
        return Err(problems);
    }
    Ok(Plan {
        entries: items
            .iter()
            .zip(converted)
            .enumerate()
            .map(|(i, (item, columns))| Planned {
                key: item.key.clone(),
                id: Uuid::new_v4(),
                locale: item.locale.clone(),
                group: group_of[i],
                source: source_of[i],
                original_url: item.original_url.clone(),
                columns: columns.unwrap_or_default(),
            })
            .collect(),
    })
}

/// 書けなかった理由（このときはトランザクションを確定しない）。
#[derive(Debug)]
pub enum Failed {
    /// 同じロケールのスラッグを他の記事が使っている（バッチの中どうしを含む）。
    SlugTaken(String),
    /// 公開の検査に通らない。
    Refused(String, Refused),
}

/// 計画どおりに作成・公開し、URLの変更履歴と翻訳の元を記録する。ワークスペースのロールのトランザクションの中で呼ぶ。
#[allow(clippy::too_many_arguments)]
pub async fn apply(
    conn: &mut PgConnection,
    manifest: &Manifest,
    scripts: &publishing::Scripts<'_>,
    schema_version: i32,
    ct: &ContentType,
    plan: Plan,
    actor: Uuid,
    limits: publishing::VersionLimits,
) -> Result<Result<Vec<Imported>, Failed>> {
    let mut out: Vec<Imported> = Vec::with_capacity(plan.entries.len());
    for p in &plan.entries {
        if let Some((_, Column::Text(Some(slug)))) = ct
            .fields
            .iter()
            .find(|f| f.kind == FieldType::Slug)
            .and_then(|f| p.columns.iter().find(|(n, _)| *n == f.name))
            && entries::slug_taken(conn, ct, &p.locale, slug, p.id).await?
        {
            return Ok(Err(Failed::SlugTaken(p.key.clone())));
        }
        // 画像のフィールド: このワークスペースにある、削除されていないメディアだけ（INV-IMG-03）
        for f in ct.fields.iter().filter(|f| f.kind == FieldType::Image) {
            let Some((_, Column::Json(Some(json)))) = p.columns.iter().find(|(n, _)| *n == f.name)
            else {
                continue;
            };
            let id = serde_json::from_str::<Value>(json)
                .ok()
                .as_ref()
                .and_then(fields::image_media_id);
            let found = match id {
                Some(id) => crate::media::store::exists(conn, id).await?,
                None => false,
            };
            if !found {
                return Ok(Err(Failed::Refused(
                    p.key.clone(),
                    Refused::Invalid(vec![format!("fields.{}: unknown media", f.name)]),
                )));
            }
        }
        entries::create(conn, ct, p.id, &p.locale, p.group, actor, &p.columns).await?;
        let published = match publishing::publish(
            conn,
            manifest,
            scripts,
            schema_version,
            p.id,
            1,
            false,
            actor,
            limits,
        )
        .await?
        {
            Ok(published) => published,
            Err(r) => return Ok(Err(Failed::Refused(p.key.clone(), r))),
        };
        if published.url != p.original_url {
            sqlx::query(
                "INSERT INTO url_changes (id, entry_id, from_url, to_url, version_id) VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(Uuid::new_v4())
            .bind(p.id)
            .bind(&p.original_url)
            .bind(&published.url)
            .bind(published.version_id)
            .execute(&mut *conn)
            .await?;
        }
        out.push(Imported {
            key: p.key.clone(),
            id: p.id,
            url: published.url,
            version_id: published.version_id,
        });
    }
    for (i, p) in plan.entries.iter().enumerate() {
        if let Some(s) = p.source {
            sqlx::query(
                "UPDATE entries SET source_entry_id = $2, reflected_source_version_id = $3 WHERE id = $1",
            )
            .bind(out[i].id)
            .bind(out[s].id)
            .bind(out[s].version_id)
            .execute(&mut *conn)
            .await?;
        }
    }
    Ok(Ok(out))
}
