//! AI向けのMarkdown版の、グループのフィールドの部分（07-ai-delivery.md、Q12）。
//!
//! グループごとに `## <フィールド名>` の節を作り、件ごとに `### <最初の text の値> {#<件のID>}` の見出しを付ける。
//! 見出しの下に、中のフィールドを「- 名前: 値」の行で、テンプレートの順に並べる。
//! - 画像は配信のURL（削除されたメディアは載せない）、参照は相手の記事のURL（相手も素材になる記事だけ。INV-AIX-01）。
//! - 文字はすべて [`escape_markdown`] でエスケープする。件のIDは [`groups::item_id`] で形を確かめたものだけを使う。

use std::fmt::Write as _;

use anyhow::Result;
use serde_json::{Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::ai::{escape_markdown, groups_of, material_urls, text_of};
use super::data::{self, Entry};
use crate::content::groups;
use crate::schema::manifest::{Field, FieldType, Manifest};

/// 値を出すのに要るもの。
pub(super) struct Context<'a> {
    pub manifest: &'a Manifest,
    pub media_base: &'a str,
    pub workspace_id: Uuid,
    pub entry: &'a Entry,
}

/// グループのフィールドの節（件がなければ None）。
pub(super) async fn section(
    conn: &mut PgConnection,
    cx: &Context<'_>,
    field: &Field,
) -> Result<Option<String>> {
    let mut out = String::new();
    for item in groups::items(cx.entry.fields.get(&field.name)) {
        let Some(id) = groups::item_id(item) else {
            continue;
        };
        let heading = field
            .fields
            .iter()
            .filter(|f| f.kind == FieldType::Text)
            .find_map(|f| text_of(item, &f.name))
            .map_or_else(|| id.to_owned(), |t| escape_markdown(&one_line(t)));
        let _ = write!(out, "\n### {heading} {{#{id}}}\n\n");
        for inner in &field.fields {
            if let Some(value) = value_text(conn, cx, inner, item).await? {
                let _ = writeln!(out, "- {}: {value}", escape_markdown(&inner.name));
            }
        }
    }
    if out.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!("## {}\n{out}", escape_markdown(&field.name))))
}

/// 改行を空白にする（見出しと箇条書きの1行に収めるため）。
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 中のフィールドの値の文字（空なら None）。
async fn value_text(
    conn: &mut PgConnection,
    cx: &Context<'_>,
    f: &Field,
    item: &Map<String, Value>,
) -> Result<Option<String>> {
    let Some(value) = item.get(&f.name).filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    Ok(match f.kind {
        FieldType::Text => value
            .as_str()
            .map(|s| escape_markdown(&one_line(s)))
            .filter(|s| !s.is_empty()),
        FieldType::Number | FieldType::Boolean => Some(value.to_string()),
        FieldType::Date | FieldType::Datetime => value.as_str().map(escape_markdown),
        FieldType::Select => {
            let values: Vec<String> = match value {
                Value::Array(a) => a
                    .iter()
                    .filter_map(Value::as_str)
                    .map(escape_markdown)
                    .collect(),
                v => v.as_str().map(escape_markdown).into_iter().collect(),
            };
            (!values.is_empty()).then(|| values.join(", "))
        }
        FieldType::Image => data::image(conn, cx.media_base, cx.workspace_id, value)
            .await?
            .map(|img| {
                let alt = img.alt.as_deref().map(one_line).unwrap_or_default();
                format!("![{}]({})", escape_markdown(&alt), img.url)
            }),
        FieldType::Reference => {
            let target = f.reference_to.as_deref().unwrap_or_default();
            let urls = material_urls(
                conn,
                cx.manifest,
                cx.entry,
                target,
                &groups_of(item, &f.name),
            )
            .await?;
            (!urls.is_empty()).then(|| urls.join(", "))
        }
        // グループの中に置けない型
        FieldType::Slug | FieldType::Richtext | FieldType::Group => None,
    })
}
