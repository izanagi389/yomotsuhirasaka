//! グループのフィールドの値の検査と、各件の固定のID（02-schema-and-delivery-api.md「フィールドの型」、Q10・Q16）。
//!
//! 値は件の配列で、件は「中のフィールド名 → 値」のオブジェクト。中の値は、それぞれのフィールドの型に従って
//! 保存のときに検証する（INV-FLD-02）。必須の検査は、ほかのフィールドと同じく公開のときに行う。
//!
//! 各件には、サーバーが `id`（見出しのアンカーと同じ形。先頭は `i-`）を振る。送られた `id` が正しい形で、
//! 一緒に保存するグループの中で重ならなければ、そのまま残す（並べ替えや編集をしても変わらない）。

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use super::{body, fields};
use crate::schema::manifest::{Field, FieldType, GROUP_ITEM_ID_KEY};

/// 各件のIDの接頭辞（見出しのアンカーは `h`）。
const ITEM_ID_PREFIX: &str = "i";

/// グループの値を検査し、そろえた値（件の配列。空なら `Null`）にする。問題があれば `problems` に足して `None`。
/// `ids` は、一緒に保存するほかのグループで使ったID（グループをまたいでも重ねない）。
pub fn normalize(
    path: &str,
    field: &Field,
    value: &Value,
    ids: &mut BTreeSet<String>,
    problems: &mut Vec<String>,
) -> Option<Value> {
    let items = match value {
        Value::Null => return Some(Value::Null),
        Value::Array(items) => items,
        _ => {
            problems.push(format!("{path}: must be a list of items"));
            return None;
        }
    };
    let max = field.group_max_items();
    if items.len() > usize::try_from(max).unwrap_or(usize::MAX) {
        problems.push(format!("{path}: must have at most {max} items"));
        return None;
    }
    let before = problems.len();
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let item_path = format!("{path}[{i}]");
        let Value::Object(obj) = item else {
            problems.push(format!("{item_path}: must be an object"));
            continue;
        };
        let mut normalized = inner_values(&item_path, field, obj, problems);
        normalized.insert(
            GROUP_ITEM_ID_KEY.to_owned(),
            Value::String(assign_id(obj.get(GROUP_ITEM_ID_KEY), ids)),
        );
        out.push(Value::Object(normalized));
    }
    (problems.len() == before).then_some(Value::Array(out))
}

/// 1件の中の値を検査する。空の値（`null`）は持たない。
fn inner_values(
    item_path: &str,
    field: &Field,
    obj: &Map<String, Value>,
    problems: &mut Vec<String>,
) -> Map<String, Value> {
    let mut out = Map::new();
    for (key, v) in obj {
        if key == GROUP_ITEM_ID_KEY {
            continue;
        }
        let Some(inner) = field.inner(key) else {
            problems.push(format!("{item_path}.{key}: unknown field"));
            continue;
        };
        match fields::value_of(inner, v) {
            Ok(Value::Null) => {}
            Ok(normalized) => {
                out.insert(key.clone(), normalized);
            }
            Err(why) => problems.push(format!("{item_path}.{key}: {why}")),
        }
    }
    out
}

/// 送られたIDが正しい形で、まだ使われていなければ残す。そうでなければ新しく振る。
fn assign_id(sent: Option<&Value>, ids: &mut BTreeSet<String>) -> String {
    if let Some(id) = sent.and_then(Value::as_str)
        && body::valid_anchor(id)
        && ids.insert(id.to_owned())
    {
        return id.to_owned();
    }
    loop {
        let id = body::new_anchor_with_prefix(ITEM_ID_PREFIX);
        if ids.insert(id.clone()) {
            return id;
        }
    }
}

/// フィールドの値の1つ（グループの中の値を含む）。
pub struct Leaf<'a> {
    /// 場所（`fields.<名前>`、グループの中は `fields.<名前>[<件>].<中の名前>`）
    pub path: String,
    /// 件の番号を除いた場所（`fields.<名前>[].<中の名前>`）。同じフィールドの値をまとめるのに使う
    pub pattern: String,
    pub field: &'a Field,
    pub value: &'a Value,
}

/// フィールドの空でない値を、グループの中まで1つずつ（グループそのものは返さない）。
pub fn leaves<'a>(fields: &'a [Field], values: &'a Map<String, Value>) -> Vec<Leaf<'a>> {
    let mut out = Vec::new();
    for f in fields {
        let Some(value) = values.get(&f.name).filter(|v| !v.is_null()) else {
            continue;
        };
        if f.kind != FieldType::Group {
            let path = format!("fields.{}", f.name);
            out.push(Leaf {
                pattern: path.clone(),
                path,
                field: f,
                value,
            });
            continue;
        }
        for (i, item) in items(Some(value)).enumerate() {
            for inner in &f.fields {
                if let Some(value) = item.get(&inner.name).filter(|v| !v.is_null()) {
                    out.push(Leaf {
                        path: format!("fields.{}[{i}].{}", f.name, inner.name),
                        pattern: format!("fields.{}[].{}", f.name, inner.name),
                        field: inner,
                        value,
                    });
                }
            }
        }
    }
    out
}

/// 必須なのに空の、グループの中のフィールド（件の番号と、中のフィールドの名前）。
pub fn missing_required<'a>(
    field: &'a Field,
    value: Option<&'a Value>,
) -> impl Iterator<Item = (usize, &'a str)> {
    items(value).enumerate().flat_map(move |(i, item)| {
        field
            .fields
            .iter()
            .filter(move |inner| inner.required && fields::is_empty(item.get(&inner.name)))
            .map(move |inner| (i, inner.name.as_str()))
    })
}

/// 件の固定のID（保存のときに振った形のものだけ）。
pub fn item_id(item: &Map<String, Value>) -> Option<&str> {
    item.get(GROUP_ITEM_ID_KEY)
        .and_then(Value::as_str)
        .filter(|id| body::valid_anchor(id))
}

/// すべてのグループの、IDのある件（グループのフィールド名, 件のID, 件）。フィールドの順、件の順。
fn identified_items<'a>(
    fields: &'a [Field],
    values: &'a Map<String, Value>,
) -> impl Iterator<Item = (&'a str, &'a str, &'a Map<String, Value>)> {
    fields
        .iter()
        .filter(|f| f.kind == FieldType::Group)
        .flat_map(move |f| {
            items(values.get(&f.name))
                .filter_map(move |item| Some((f.name.as_str(), item_id(item)?, item)))
        })
}

/// グループの各件のID（フィールドの順、件の順）。見出しのアンカーと同じく版に記録し、消えたアンカーの一覧に使う（Q12）。
pub fn item_ids(fields: &[Field], values: &Map<String, Value>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    identified_items(fields, values)
        .filter(|(_, id, _)| seen.insert(*id))
        .map(|(_, id, _)| id.to_owned())
        .collect()
}

/// 前の版と比べて、増えた・消えた・中身が変わったグループの件のID（「訂正として公開」の影響したセクション。Q12）。
/// 件はグループのフィールド名とIDの組で見分ける（別のグループに同じIDがあっても数え落とさない）。
pub fn changed_items(
    fields: &[Field],
    before: &Map<String, Value>,
    after: &Map<String, Value>,
) -> BTreeSet<String> {
    type Key<'a> = (&'a str, &'a str);
    let keyed = |values| -> BTreeMap<Key<'_>, &Map<String, Value>> {
        identified_items(fields, values)
            .map(|(field, id, item)| ((field, id), item))
            .collect()
    };
    let (old, new) = (keyed(before), keyed(after));
    old.keys()
        .chain(new.keys())
        .filter(|key| old.get(*key) != new.get(*key))
        .map(|(_, id)| (*id).to_owned())
        .collect()
}

/// 保存済みのグループの値の件（配列でなければ空）。
pub fn items(value: Option<&Value>) -> impl Iterator<Item = &Map<String, Value>> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn faq() -> Field {
        serde_json::from_value(json!({
            "name": "faq", "type": "group", "maxItems": 3,
            "fields": [
                {"name": "question", "type": "text", "required": true},
                {"name": "position", "type": "number", "integer": true, "min": 1, "max": 10},
                {"name": "level", "type": "select", "options": [{"value": "basic"}]}
            ]
        }))
        .unwrap()
    }

    fn run(v: &Value) -> Result<Value, Vec<String>> {
        let mut problems = Vec::new();
        normalize("fields.faq", &faq(), v, &mut BTreeSet::new(), &mut problems).ok_or(problems)
    }

    #[test]
    fn items_get_ids_that_survive_reordering() {
        let first = run(&json!([{"question": "a"}, {"question": "b", "level": null}])).unwrap();
        let items: Vec<_> = items(Some(&first)).cloned().collect();
        let id = |i: usize| items[i]["id"].as_str().unwrap().to_owned();
        assert!(id(0).starts_with("i-") && id(0) != id(1));
        assert!(!items[1].contains_key("level"), "null は持たない");

        // 並べ替えて送り直しても、IDは変わらない
        let again = run(&json!([items[1], items[0]])).unwrap();
        assert_eq!(again[0]["id"], json!(id(1)));
        assert_eq!(again[1]["id"], json!(id(0)));

        // 重なるIDや正しくない形のIDは、振り直す
        let dup = run(&json!([
            {"id": id(0), "question": "a"},
            {"id": id(0), "question": "b"},
            {"id": "a b", "question": "c"}
        ]))
        .unwrap();
        assert_eq!(dup[0]["id"], json!(id(0)));
        assert_ne!(dup[1]["id"], json!(id(0)));
        assert_ne!(dup[2]["id"], json!("a b"));
    }

    #[test]
    fn ids_are_not_shared_between_groups_saved_together() {
        let mut ids = BTreeSet::new();
        let sent = json!([{"id": "i-aaaaaaaaaa", "question": "a"}]);
        let mut problems = Vec::new();
        let first = normalize("fields.faq", &faq(), &sent, &mut ids, &mut problems).unwrap();
        let second = normalize("fields.tips", &faq(), &sent, &mut ids, &mut problems).unwrap();
        assert_eq!(first[0]["id"], json!("i-aaaaaaaaaa"));
        assert_ne!(
            second[0]["id"],
            json!("i-aaaaaaaaaa"),
            "別のグループで使ったIDは振り直す"
        );
        assert!(problems.is_empty());
    }

    #[test]
    fn same_id_in_two_groups_is_counted_per_group() {
        let mut tips = faq();
        tips.name = "tips".into();
        let fields = vec![faq(), tips];
        let item = |q: &str| json!([{"id": "i-aaaaaaaaaa", "question": q}]);
        let before = json!({"faq": item("a"), "tips": item("t")});
        let after = json!({"faq": item("a2"), "tips": item("t")});
        let (before, after) = (before.as_object().unwrap(), after.as_object().unwrap());
        assert_eq!(
            changed_items(&fields, before, after)
                .into_iter()
                .collect::<Vec<_>>(),
            ["i-aaaaaaaaaa"]
        );
        assert_eq!(
            item_ids(&fields, after),
            ["i-aaaaaaaaaa"],
            "版に記録するIDは重ねない"
        );
    }

    #[test]
    fn item_ids_and_changed_items() {
        let fields = vec![faq()];
        let values = |v: Value| json!({ "faq": v }).as_object().unwrap().clone();
        let before = values(json!([
            {"id": "i-aaaaaaaaaa", "question": "a"},
            {"id": "i-bbbbbbbbbb", "question": "b"},
            {"id": "i-cccccccccc", "question": "c"}
        ]));
        let after = values(json!([
            {"id": "i-bbbbbbbbbb", "question": "b"},
            {"id": "i-aaaaaaaaaa", "question": "a2"},
            {"id": "i-dddddddddd", "question": "d"}
        ]));
        assert_eq!(
            item_ids(&fields, &after),
            ["i-bbbbbbbbbb", "i-aaaaaaaaaa", "i-dddddddddd"]
        );
        // 並べ替えだけの件（b）は変わっていない
        let changed: Vec<String> = changed_items(&fields, &before, &after)
            .into_iter()
            .collect();
        assert_eq!(changed, ["i-aaaaaaaaaa", "i-cccccccccc", "i-dddddddddd"]);
    }

    #[test]
    fn inner_values_follow_their_types() {
        for bad in [
            json!({"question": 1}),
            json!({"position": 11}),
            json!({"position": 1.5}),
            json!({"level": "expert"}),
            json!({"unknown": "SECRET-VALUE"}),
        ] {
            let problems = run(&json!([{"question": "ok"}, bad])).unwrap_err();
            assert!(
                problems.iter().all(|p| p.starts_with("fields.faq[1].")),
                "{problems:?}"
            );
            assert!(problems.iter().all(|p| !p.contains("SECRET-VALUE")));
        }
        assert!(run(&json!({"question": "a"})).is_err());
        assert!(run(&json!(["a"])).is_err());
        assert!(run(&json!([{}, {}, {}, {}])).is_err(), "maxItems");
        assert_eq!(run(&Value::Null).unwrap(), Value::Null);
        assert_eq!(run(&json!([])).unwrap(), json!([]));
        assert_eq!(faq().kind, FieldType::Group);
    }
}
