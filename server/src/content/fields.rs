//! 下書きのフィールドの値の検査と、DBの列への変換（Q201）。
//!
//! 下書きは書きかけでよいので、必須のフィールドが空でも保存できる（必須は公開のときに確かめる）。
//! 問題の説明には、フィールド名と理由だけを書き、値は書かない。

use serde_json::{Map, Value};
use uuid::Uuid;

use super::body;
use crate::schema::manifest::{ContentType, FieldType, Manifest};

const MAX_TEXT_CHARS: usize = 10_000;
const MAX_SLUG_LEN: usize = 200;
const MAX_REFERENCES: usize = 100;

/// DBの列に書く値。`None` は空（NULL）。
#[derive(Debug, Clone, PartialEq)]
pub enum Column {
    Text(Option<String>),
    /// JSONの文字列（`::jsonb` で書く）
    Json(Option<String>),
    Uuid(Option<Uuid>),
    Uuids(Option<Vec<Uuid>>),
}

impl Column {
    /// 列の型（SQLのキャストに使う固定の文字列）。
    pub fn sql_type(&self) -> &'static str {
        match self {
            Self::Text(_) => "text",
            Self::Json(_) => "jsonb",
            Self::Uuid(_) => "uuid",
            Self::Uuids(_) => "uuid[]",
        }
    }
}

/// スラッグ: 英小文字・数字をハイフンでつないだもの。
pub fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_SLUG_LEN
        && s.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

fn valid_text(s: &str) -> bool {
    s.chars().count() <= MAX_TEXT_CHARS
        && !s.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
}

/// 1つのフィールドの値を検査して列の値にする。リッチテキストはアンカーIDを付けた本文にする。
fn convert(
    manifest: &Manifest,
    ct: &ContentType,
    name: &str,
    value: &Value,
    problems: &mut Vec<String>,
) -> Option<(String, Column)> {
    let Some(field) = ct.field(name) else {
        problems.push(format!("fields.{name}: unknown field"));
        return None;
    };
    let path = format!("fields.{name}");
    let column = match (field.kind, value) {
        (FieldType::Text, Value::Null) | (FieldType::Slug, Value::Null) => Column::Text(None),
        (FieldType::Text, Value::String(s)) => {
            if !valid_text(s) {
                problems.push(format!(
                    "{path}: text is too long or contains control characters"
                ));
                return None;
            }
            Column::Text(Some(s.clone()))
        }
        (FieldType::Slug, Value::String(s)) => {
            if !valid_slug(s) {
                problems.push(format!(
                    "{path}: slug must be a-z and 0-9 joined by single hyphens"
                ));
                return None;
            }
            Column::Text(Some(s.clone()))
        }
        (FieldType::Richtext, Value::Null) => Column::Json(None),
        (FieldType::Richtext, doc) => match body::validate(doc, &manifest.body) {
            Ok(normalized) => Column::Json(Some(normalized.to_string())),
            Err(found) => {
                problems.extend(
                    found
                        .into_iter()
                        .map(|p| format!("{path}{}", p.trim_start_matches('$'))),
                );
                return None;
            }
        },
        (FieldType::Reference, Value::Null) if field.multiple => Column::Uuids(None),
        (FieldType::Reference, Value::Null) => Column::Uuid(None),
        (FieldType::Reference, Value::String(s)) if !field.multiple => match Uuid::parse_str(s) {
            Ok(id) => Column::Uuid(Some(id)),
            Err(_) => {
                problems.push(format!("{path}: must be a translation group ID"));
                return None;
            }
        },
        (FieldType::Reference, Value::Array(items)) if field.multiple => {
            let ids: Option<Vec<Uuid>> = items
                .iter()
                .map(|v| v.as_str().and_then(|s| Uuid::parse_str(s).ok()))
                .collect();
            match ids {
                Some(ids) if ids.len() <= MAX_REFERENCES => Column::Uuids(Some(ids)),
                _ => {
                    problems.push(format!(
                        "{path}: must be at most {MAX_REFERENCES} translation group IDs"
                    ));
                    return None;
                }
            }
        }
        (FieldType::Image, Value::Null) => Column::Json(None),
        (FieldType::Image, Value::Object(obj)) => match image_value(obj) {
            Ok(v) => Column::Json(Some(v.to_string())),
            Err(why) => {
                problems.push(format!("{path}: {why}"));
                return None;
            }
        },
        _ => {
            problems.push(format!("{path}: wrong type of value"));
            return None;
        }
    };
    Some((field.name.clone(), column))
}

/// 画像のフィールドの値を検査し、そろえた形（`mediaId` は小文字・ハイフン区切り）にする。
/// メディアがワークスペースにあるかは、保存のときにデータと照らし合わせて確かめる（INV-IMG-03）。
fn image_value(obj: &Map<String, Value>) -> Result<Value, &'static str> {
    if obj
        .keys()
        .any(|k| !matches!(k.as_str(), "mediaId" | "alt" | "decorative"))
    {
        return Err("image may only have mediaId, alt and decorative");
    }
    let id = obj
        .get("mediaId")
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or("image needs a media ID")?;
    let mut out = Map::new();
    out.insert("mediaId".into(), Value::String(id.hyphenated().to_string()));
    match obj.get("alt") {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) if valid_text(s) => {
            out.insert("alt".into(), Value::String(s.clone()));
        }
        Some(_) => return Err("alt must be text without control characters"),
    }
    match obj.get("decorative") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => {}
        Some(Value::Bool(true)) => {
            out.insert("decorative".into(), Value::Bool(true));
        }
        Some(_) => return Err("decorative must be true or false"),
    }
    Ok(Value::Object(out))
}

/// 画像のフィールドの値の、メディアのID（そろえた値から読む）。
pub fn image_media_id(v: &Value) -> Option<Uuid> {
    v.get("mediaId")
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok())
}

/// 送られたフィールド（一部でよい）を検査し、(フィールド名, 列の値) の一覧にする。
pub fn convert_all(
    manifest: &Manifest,
    ct: &ContentType,
    values: &Map<String, Value>,
) -> Result<Vec<(String, Column)>, Vec<String>> {
    let mut problems = Vec::new();
    let mut out = Vec::new();
    for (name, value) in values {
        if let Some(c) = convert(manifest, ct, name, value, &mut problems) {
            out.push(c);
        }
    }
    if problems.is_empty() {
        Ok(out)
    } else {
        Err(problems)
    }
}

/// 値が空か（必須の検査に使う。公開のときと移行のとき）。
pub fn is_empty(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.is_empty(),
        Some(Value::Array(a)) => a.is_empty(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::manifest::tests::sample;
    use serde_json::json;

    fn conv(v: Value) -> Result<Vec<(String, Column)>, Vec<String>> {
        let m = sample();
        let ct = m.content_types[0].clone();
        convert_all(&m, &ct, v.as_object().unwrap())
    }

    #[test]
    fn converts_values() {
        let id = Uuid::new_v4();
        let out = conv(json!({
            "title": "t",
            "slug": "hello-world-2",
            "related": [id.to_string()],
            "body": {"type": "doc", "content": [{"type": "paragraph"}]}
        }))
        .unwrap();
        assert!(out.contains(&("title".into(), Column::Text(Some("t".into())))));
        assert!(out.contains(&("related".into(), Column::Uuids(Some(vec![id])))));
        assert!(conv(json!({"title": null, "related": null})).is_ok());
    }

    #[test]
    fn normalizes_image_values() {
        let v = image_value(
            json!({"mediaId": "0123456789ABCDEF0123456789ABCDEF", "alt": "a", "decorative": false})
                .as_object()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            v,
            json!({"mediaId": "01234567-89ab-cdef-0123-456789abcdef", "alt": "a"})
        );
        assert_eq!(
            image_media_id(&v),
            Uuid::parse_str("0123456789abcdef0123456789abcdef").ok()
        );
        for bad in [
            json!({}),
            json!({"mediaId": "x"}),
            json!({"mediaId": Uuid::new_v4().to_string(), "src": "https://evil.example/a.png"}),
            json!({"mediaId": Uuid::new_v4().to_string(), "alt": 1}),
            json!({"mediaId": Uuid::new_v4().to_string(), "alt": "a\u{7}"}),
            json!({"mediaId": Uuid::new_v4().to_string(), "decorative": "yes"}),
        ] {
            assert!(image_value(bad.as_object().unwrap()).is_err(), "{bad}");
        }
    }

    #[test]
    fn rejects_bad_values_without_echoing_them() {
        for bad in [
            json!({"nope": "x"}),
            json!({"slug": "Hello"}),
            json!({"slug": "a--b"}),
            json!({"slug": "-a"}),
            json!({"title": 1}),
            json!({"title": "a\u{0}b"}),
            json!({"related": "not-an-array"}),
            json!({"related": ["SECRET-VALUE"]}),
            json!({"body": {"type": "doc", "content": [{"type": "script"}]}}),
        ] {
            let problems = conv(bad.clone()).unwrap_err();
            assert!(!problems.is_empty(), "{bad}");
            assert!(problems.iter().all(|p| !p.contains("SECRET-VALUE")));
        }
    }

    #[test]
    fn slugs() {
        assert!(valid_slug("a") && valid_slug("hello-world-2"));
        assert!(
            !valid_slug("")
                && !valid_slug("Hello")
                && !valid_slug("a_b")
                && !valid_slug("a-")
                && !valid_slug("日本")
        );
    }
}
