//! 配信APIのデータの読み取り（ワークスペースのロールに切り替えたトランザクションの中で呼ぶ）。
//!
//! - 返すのは**公開中の版だけ**。下書きは読まない（INV-PUB-01）。公開中かどうかは、読むたびに `entries.published_version_id`
//!   と照合する（保存済みの状態やキャッシュを信用しない。原則3、INV-PUB-02）。削除した記事は返さない。
//! - 参照フィールドは翻訳グループを指し、同じロケールの公開中の記事に解決する（INV-LOC-04）。
//! - 他のロケールへのリンクには、公開中の記事だけを含める（INV-LOC-03）。
//! - 公開URLは記事のいまのURL（`entries.url`）。依存先の変化で再計算されると、版を作らずに変わる（Q39）。
//! - 例外はプレビュー（`draft`）: プレビュー用トークンの対象の記事1件だけを、下書きから作る（INV-PRV-01）。

use std::collections::HashMap;

use anyhow::Result;
use serde_json::{Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::content::body::IMAGE;
use crate::content::entries;
use crate::content::publishing::{self, Scripts};
use crate::schema::manifest::Manifest;

const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;

/// 配信する記事（公開中の版）。
#[derive(Debug, Clone)]
pub struct Entry {
    pub id: Uuid,
    pub content_type: String,
    pub locale: String,
    pub group_id: Uuid,
    pub url: String,
    pub published_at: String,
    pub first_published_at: String,
    /// 公開中の版のフィールドのスナップショット。
    pub fields: Map<String, Value>,
    /// 公開中の版の本文の変換結果（フィールド名 → {markdown, html}）。HTMLは無害化する前のもの。
    pub rendered: Map<String, Value>,
    /// 記事の公開設定（読むたびのいまの値）: 外部AI向けの素材に含めるか、学習への利用を拒否するか（07-ai-delivery.md）。
    pub include_in_ai_materials: bool,
    pub opt_out_of_ai_training: bool,
}

type Row = (
    Uuid,
    String,
    String,
    Uuid,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
    bool,
    bool,
);

fn select(filter: &str, tail: &str) -> String {
    format!(
        "SELECT e.id, e.content_type, e.locale, e.translation_group_id, e.url,
                to_char(v.published_at AT TIME ZONE 'UTC', {RFC3339}),
                to_char(f.first AT TIME ZONE 'UTC', {RFC3339}),
                v.fields::text, v.rendered::text, e.include_in_ai_materials, e.opt_out_of_ai_training
         FROM entries e
         JOIN versions v ON v.id = e.published_version_id
         CROSS JOIN LATERAL (SELECT min(published_at) AS first FROM versions WHERE entry_id = e.id) f
         WHERE e.deleted_at IS NULL AND v.fields IS NOT NULL AND {filter}
         {tail}"
    )
}

fn entry(r: Row) -> Result<Option<Entry>> {
    let (
        id,
        content_type,
        locale,
        group_id,
        url,
        published_at,
        first_published_at,
        fields,
        rendered,
        include_in_ai_materials,
        opt_out_of_ai_training,
    ) = r;
    let Some(fields) = fields else {
        return Ok(None);
    };
    let object = |text: Option<String>| -> Result<Map<String, Value>> {
        Ok(
            match text
                .map(|t| serde_json::from_str::<Value>(&t))
                .transpose()?
            {
                Some(Value::Object(m)) => m,
                _ => Map::new(),
            },
        )
    };
    let fields = object(Some(fields))?;
    let rendered = object(rendered)?;
    Ok(Some(Entry {
        id,
        content_type,
        locale,
        group_id,
        url: url.unwrap_or_default(),
        published_at,
        first_published_at,
        fields,
        rendered,
        include_in_ai_materials,
        opt_out_of_ai_training,
    }))
}

fn entries(rows: Vec<Row>) -> Result<Vec<Entry>> {
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        if let Some(e) = entry(r)? {
            out.push(e);
        }
    }
    Ok(out)
}

/// プレビューする記事（下書きから作る。記事がない・削除済みなら None）。
///
/// フィールドと本文の変換結果とURLは下書きから今計算する（変換やURLの計算に失敗したら、空・空文字にする）。
/// 公開日時は、公開したことがあればその日時、なければ下書きの更新日時。
pub async fn draft(
    conn: &mut PgConnection,
    manifest: &Manifest,
    scripts: &Scripts<'_>,
    entry_id: Uuid,
) -> Result<Option<Entry>> {
    let Some(d) = entries::draft(conn, manifest, entry_id).await? else {
        return Ok(None);
    };
    let Some(ct) = manifest.content_type(&d.content_type) else {
        return Ok(None);
    };
    let rendered = match publishing::render(scripts, manifest, ct, &d.fields).await {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    };
    let url = match publishing::compute_url(scripts, ct, d.id, &d.locale, &d.fields).await {
        Ok(Some((url, _))) => url,
        _ => String::new(),
    };
    let (latest, first, include_in_ai_materials, opt_out_of_ai_training): (
        Option<String>,
        Option<String>,
        bool,
        bool,
    ) = sqlx::query_as(crate::sql!(
        "SELECT to_char(max(v.published_at) AT TIME ZONE 'UTC', {RFC3339}),
                to_char(min(v.published_at) AT TIME ZONE 'UTC', {RFC3339}),
                e.include_in_ai_materials, e.opt_out_of_ai_training
         FROM entries e LEFT JOIN versions v ON v.entry_id = e.id
         WHERE e.id = $1
         GROUP BY e.id"
    ))
    .bind(entry_id)
    .fetch_one(conn)
    .await?;
    Ok(Some(Entry {
        id: d.id,
        content_type: d.content_type,
        locale: d.locale,
        group_id: d.translation_group_id,
        url,
        published_at: latest.unwrap_or_else(|| d.updated_at.clone()),
        first_published_at: first.unwrap_or_else(|| d.updated_at.clone()),
        fields: d.fields,
        rendered,
        include_in_ai_materials,
        opt_out_of_ai_training,
    }))
}

/// 公開URLで1件（いまのURL。公開中の記事のURLは重ならない）。
pub async fn by_url(conn: &mut PgConnection, url: &str) -> Result<Option<Entry>> {
    let row: Option<Row> = sqlx::query_as(crate::db::trusted(select("e.url = $1", "LIMIT 1")))
        .bind(url)
        .fetch_optional(conn)
        .await?;
    Ok(match row {
        Some(r) => entry(r)?,
        None => None,
    })
}

/// IDで1件。
pub async fn by_id(conn: &mut PgConnection, content_type: &str, id: Uuid) -> Result<Option<Entry>> {
    let row: Option<Row> = sqlx::query_as(crate::db::trusted(select(
        "e.id = $1 AND e.content_type = $2",
        "",
    )))
    .bind(id)
    .bind(content_type)
    .fetch_optional(conn)
    .await?;
    Ok(match row {
        Some(r) => entry(r)?,
        None => None,
    })
}

/// スラッグで1件（公開中の版のスラッグで探す）。
pub async fn by_slug(
    conn: &mut PgConnection,
    content_type: &str,
    slug_field: &str,
    slug: &str,
    locale: &str,
) -> Result<Option<Entry>> {
    let row: Option<Row> = sqlx::query_as(crate::db::trusted(select(
        "e.content_type = $1 AND e.locale = $2 AND v.fields ->> $3 = $4",
        "LIMIT 1",
    )))
    .bind(content_type)
    .bind(locale)
    .bind(slug_field)
    .bind(slug)
    .fetch_optional(conn)
    .await?;
    Ok(match row {
        Some(r) => entry(r)?,
        None => None,
    })
}

/// 一覧（最初に公開した日時の新しい順）。
pub async fn list(
    conn: &mut PgConnection,
    content_type: &str,
    locale: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<Entry>> {
    let rows: Vec<Row> = sqlx::query_as(crate::db::trusted(select(
        "e.content_type = $1 AND e.locale = $2",
        "ORDER BY f.first DESC, e.id LIMIT $3 OFFSET $4",
    )))
    .bind(content_type)
    .bind(locale)
    .bind(limit)
    .bind(offset)
    .fetch_all(conn)
    .await?;
    entries(rows)
}

/// 同じ翻訳グループの、他のロケールの公開中の記事。
pub async fn alternates(
    conn: &mut PgConnection,
    group_id: Uuid,
    except: Uuid,
) -> Result<Vec<Entry>> {
    let rows: Vec<Row> = sqlx::query_as(crate::db::trusted(select(
        "e.translation_group_id = $1 AND e.id <> $2",
        "ORDER BY e.locale",
    )))
    .bind(group_id)
    .bind(except)
    .fetch_all(conn)
    .await?;
    entries(rows)
}

/// 翻訳グループの一覧を、指定のロケール・コンテンツタイプの公開中の記事に解決する（順番は保つ。ないものは除く）。
pub async fn resolve_groups(
    conn: &mut PgConnection,
    content_type: &str,
    locale: &str,
    groups: &[Uuid],
) -> Result<Vec<Entry>> {
    if groups.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<Row> = sqlx::query_as(crate::db::trusted(select(
        "e.translation_group_id = ANY($1) AND e.content_type = $2 AND e.locale = $3",
        "",
    )))
    .bind(groups)
    .bind(content_type)
    .bind(locale)
    .fetch_all(conn)
    .await?;
    let by_group: HashMap<Uuid, Entry> = entries(rows)?
        .into_iter()
        .map(|e| (e.group_id, e))
        .collect();
    Ok(groups
        .iter()
        .filter_map(|g| by_group.get(g).cloned())
        .collect())
}

/// 記事のIDで、公開中の記事を読む（並びは渡した順。公開中でない・削除済みの記事は含めない）。
pub async fn by_ids(conn: &mut PgConnection, ids: &[Uuid]) -> Result<Vec<Entry>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<Row> = sqlx::query_as(crate::db::trusted(select("e.id = ANY($1)", "")))
        .bind(ids)
        .fetch_all(conn)
        .await?;
    let by_id: HashMap<Uuid, Entry> = entries(rows)?.into_iter().map(|e| (e.id, e)).collect();
    Ok(ids.iter().filter_map(|id| by_id.get(id).cloned()).collect())
}

/// 変換した本文の画像の参照先（`cms-media:<ID>`）を、その時点のメディアのURLに置き換える。
/// 削除されたメディアの参照先は空にする（原則3、INV-PUB-06）。
pub async fn resolve_media_refs(
    conn: &mut PgConnection,
    media_base: &str,
    workspace_id: Uuid,
    text: &str,
) -> Result<String> {
    const SCHEME: &str = "cms-media:";
    let mut ids = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(SCHEME) {
        let after = &rest[i + SCHEME.len()..];
        if let Some(id) = after.get(..36).and_then(|s| Uuid::parse_str(s).ok()) {
            ids.push(id);
        }
        rest = after;
    }
    if !text.contains(SCHEME) {
        return Ok(text.to_owned());
    }
    let rows: Vec<(Uuid, String)> = if ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as("SELECT id, file FROM media WHERE id = ANY($1) AND deleted_at IS NULL")
            .bind(&ids)
            .fetch_all(conn)
            .await?
    };
    let found: HashMap<Uuid, String> = rows.into_iter().collect();
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(SCHEME) {
        out.push_str(&rest[..i]);
        let after = &rest[i + SCHEME.len()..];
        match after.get(..36).and_then(|s| Uuid::parse_str(s).ok()) {
            Some(id) => {
                if let Some(file) = found.get(&id) {
                    out.push_str(&crate::media::store::url_of(media_base, workspace_id, file));
                }
                rest = &after[36..];
            }
            // 正しいIDの続かない参照先は、参照先として扱わない（接頭辞だけを落とす）
            None => rest = after,
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// 本文の画像に、配信のURLと大きさ（`src`・`width`・`height`）を足す。削除されたメディアの画像は本文から除く。
pub async fn with_media(
    conn: &mut PgConnection,
    media_base: &str,
    workspace_id: Uuid,
    mut doc: Value,
) -> Result<Value> {
    let ids = crate::content::body::media_ids(&doc);
    if ids.is_empty() {
        return Ok(doc);
    }
    let rows: Vec<(Uuid, String, i32, i32)> = sqlx::query_as(
        "SELECT id, file, width, height FROM media WHERE id = ANY($1) AND deleted_at IS NULL",
    )
    .bind(&ids)
    .fetch_all(conn)
    .await?;
    let found: HashMap<Uuid, (String, i32, i32)> = rows
        .into_iter()
        .map(|(id, f, w, h)| (id, (f, w, h)))
        .collect();
    fn walk(v: &mut Value, found: &HashMap<Uuid, (String, i32, i32)>, base: &str, ws: Uuid) {
        if let Some(children) = v.get_mut("content").and_then(Value::as_array_mut) {
            children.retain(|c| {
                if c.get("type").and_then(Value::as_str) != Some(IMAGE) {
                    return true;
                }
                c.get("attrs")
                    .and_then(|a| a.get("mediaId"))
                    .and_then(Value::as_str)
                    .and_then(|s| Uuid::parse_str(s).ok())
                    .is_some_and(|id| found.contains_key(&id))
            });
            for c in children.iter_mut() {
                if c.get("type").and_then(Value::as_str) == Some(IMAGE) {
                    let id = c["attrs"]["mediaId"]
                        .as_str()
                        .and_then(|s| Uuid::parse_str(s).ok());
                    if let (Some(id), Some(attrs)) =
                        (id, c.get_mut("attrs").and_then(Value::as_object_mut))
                        && let Some((file, w, h)) = found.get(&id)
                    {
                        attrs.insert(
                            "src".into(),
                            Value::String(crate::media::store::url_of(base, ws, file)),
                        );
                        attrs.insert("width".into(), Value::from(*w));
                        attrs.insert("height".into(), Value::from(*h));
                    }
                }
                walk(c, found, base, ws);
            }
        }
    }
    walk(&mut doc, &found, media_base, workspace_id);
    Ok(doc)
}
