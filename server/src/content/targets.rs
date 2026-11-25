//! フィールドが指すワークスペースのデータ（画像のメディア、参照先の記事）が、いまあるかの検査。
//!
//! 直下のフィールドとグループの中のフィールドを同じ規則で確かめる（INV-IMG-03、INV-FLD-02）。
//! 値の形は先に `fields::convert_all` で確かめておくこと。ここでは指す先があるかだけを見る。
//! 問い合わせは、フィールドごと（グループの中は、中のフィールドごとに全件まとめて）1回にする。

use std::collections::BTreeMap;

use anyhow::Result;
use serde_json::{Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::{entries, fields, groups};
use crate::schema::manifest::{Field, FieldType};

/// 1つのフィールドが指す先（グループの中なら全件ぶん）。
#[derive(Debug, Clone)]
struct Wanted {
    /// 参照先のコンテンツタイプ（画像なら `None`）
    reference_to: Option<String>,
    ids: Vec<Uuid>,
    /// メディアのIDを読めない画像の値があった
    unreadable: bool,
}

/// フィールドの値が指す先の一覧。保存の検査と取り込みで使う。
#[derive(Debug, Clone, Default)]
pub struct Targets {
    /// 件の番号を除いた場所（`fields.<名前>`・`fields.<名前>[].<中の名前>`）ごと
    by_pattern: BTreeMap<String, Wanted>,
}

/// 参照のフィールドの値（1つでも配列でも）の、翻訳グループのID。
fn reference_ids(value: &Value) -> Vec<Uuid> {
    let one = |v: &Value| v.as_str().and_then(|s| Uuid::parse_str(s).ok());
    match value {
        Value::Array(items) => items.iter().filter_map(one).collect(),
        v => one(v).into_iter().collect(),
    }
}

impl Targets {
    pub fn of(fields: &[Field], values: &Map<String, Value>) -> Self {
        let mut by_pattern: BTreeMap<String, Wanted> = BTreeMap::new();
        for leaf in groups::leaves(fields, values) {
            let (reference_to, ids, unreadable) = match leaf.field.kind {
                FieldType::Image => {
                    let id = fields::image_media_id(leaf.value);
                    (None, id.into_iter().collect(), id.is_none())
                }
                FieldType::Reference => (
                    Some(leaf.field.reference_to.clone().unwrap_or_default()),
                    reference_ids(leaf.value),
                    false,
                ),
                _ => continue,
            };
            let wanted = by_pattern.entry(leaf.pattern).or_insert(Wanted {
                reference_to,
                ids: Vec::new(),
                unreadable: false,
            });
            wanted.ids.extend(ids);
            wanted.unreadable |= unreadable;
        }
        Self { by_pattern }
    }

    /// 参照のフィールドの場所（取り込みでは参照を受け付けない）。
    pub fn references(&self) -> impl Iterator<Item = &str> {
        self.by_pattern
            .iter()
            .filter(|(_, w)| w.reference_to.is_some())
            .map(|(pattern, _)| pattern.as_str())
    }

    /// 指す先が見つからないフィールドの問題（`<場所>: unknown media`・`<場所>: unknown entry`）。
    pub async fn unknown(&self, conn: &mut PgConnection) -> Result<Vec<String>> {
        let mut problems = Vec::new();
        for (pattern, wanted) in &self.by_pattern {
            let (found, what) = match &wanted.reference_to {
                None => (
                    !wanted.unreadable && crate::media::store::all_exist(conn, &wanted.ids).await?,
                    "media",
                ),
                Some(to) => (entries::groups_exist(conn, to, &wanted.ids).await?, "entry"),
            };
            if !found {
                problems.push(format!("{pattern}: unknown {what}"));
            }
        }
        Ok(problems)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn targets_include_values_inside_groups() {
        let fields: Vec<Field> = serde_json::from_value(json!([
            {"name": "cover", "type": "image"},
            {"name": "see", "type": "reference", "referenceTo": "post", "multiple": true},
            {"name": "faq", "type": "group", "fields": [
                {"name": "figure", "type": "image"},
                {"name": "link", "type": "reference", "referenceTo": "page"}
            ]}
        ]))
        .unwrap();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let values = json!({
            "cover": {"mediaId": a.to_string()},
            "see": [b.to_string()],
            "faq": [
                {"figure": {"mediaId": b.to_string()}, "link": a.to_string()},
                {"figure": {"alt": "x"}}
            ]
        });
        let t = Targets::of(&fields, values.as_object().unwrap());
        assert_eq!(t.by_pattern["fields.cover"].ids, vec![a]);
        assert_eq!(t.by_pattern["fields.see"].ids, vec![b]);
        assert_eq!(t.by_pattern["fields.faq[].figure"].ids, vec![b]);
        assert!(t.by_pattern["fields.faq[].figure"].unreadable);
        assert_eq!(
            t.by_pattern["fields.faq[].link"].reference_to.as_deref(),
            Some("page")
        );
        assert_eq!(
            t.references().collect::<Vec<_>>(),
            ["fields.faq[].link", "fields.see"]
        );
    }
}
