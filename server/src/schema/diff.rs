//! 2つのマニフェストの差分と、破壊的な変更の判定（Q53、INV-SCH-02）。
//!
//! 破壊的な変更（既存のデータを消す・読めなくする・満たさなくなる変更）は、明示的な指定がなければデプロイを拒否する。
//! 「追加 → データ移行 → 削除」の段階を踏んでもらうため。

use std::collections::BTreeSet;

use super::manifest::{Field, Manifest, types_by_name};

/// 破壊的な変更の1つ。`describe` で人が読める説明にする。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Breaking {
    ContentTypeRemoved(String),
    FieldRemoved {
        content_type: String,
        field: String,
    },
    FieldTypeChanged {
        content_type: String,
        field: String,
    },
    FieldBecameRequired {
        content_type: String,
        field: String,
    },
    /// 受け付ける値が狭まる（選択肢を外す、範囲を狭める、グループの件数の上限を下げる、グループの中のフィールドを
    /// 外す・型を変える・必須にする、など。2026-11-25）。
    FieldNarrowed {
        content_type: String,
        field: String,
    },
    RequiredFieldAdded {
        content_type: String,
        field: String,
    },
    LocaleRemoved(String),
    BodyNodeRemoved(String),
    BodyMarkRemoved(String),
    EmbedProviderRemoved(String),
}

impl Breaking {
    pub fn describe(&self) -> String {
        match self {
            Self::ContentTypeRemoved(t) => format!("content type `{t}` is removed"),
            Self::FieldRemoved {
                content_type,
                field,
            } => {
                format!("field `{content_type}.{field}` is removed")
            }
            Self::FieldTypeChanged {
                content_type,
                field,
            } => {
                format!("field `{content_type}.{field}` changes its type")
            }
            Self::FieldBecameRequired {
                content_type,
                field,
            } => {
                format!("field `{content_type}.{field}` becomes required")
            }
            Self::FieldNarrowed {
                content_type,
                field,
            } => {
                format!("field `{content_type}.{field}` narrows the values it accepts")
            }
            Self::RequiredFieldAdded {
                content_type,
                field,
            } => {
                format!("required field `{content_type}.{field}` is added")
            }
            Self::LocaleRemoved(l) => format!("locale `{l}` is removed"),
            Self::BodyNodeRemoved(n) => format!("body node `{n}` is removed"),
            Self::BodyMarkRemoved(n) => format!("body mark `{n}` is removed"),
            Self::EmbedProviderRemoved(h) => format!("embed provider `{h}` is removed"),
        }
    }
}

/// 列の形が変わるか（型・複数か・整数か・参照先）。変わるなら、列を作り直す（中身は消える）。
/// グループの中のフィールドでは、その値を消すかどうかの判定に使う。
pub fn shape_changed(old: &Field, new: &Field) -> bool {
    old.kind != new.kind
        || old.multiple != new.multiple
        || old.integer != new.integer
        || old.reference_to != new.reference_to
}

/// 型を変えずに、受け付ける値が狭まるか（2026-11-25。合わない値があれば移行で止める）。
pub fn narrowed(old: &Field, new: &Field) -> bool {
    // 新しい限界が、ないところに加わるか、前より内側に動いたか
    let tightened =
        |old: Option<f64>, new: Option<f64>, inside: fn(f64, f64) -> bool| match (old, new) {
            (_, None) => false,
            (None, Some(_)) => true,
            (Some(o), Some(n)) => inside(n, o),
        };
    let ((old_min, old_max), (new_min, new_max)) = (old.bounds(), new.bounds());
    let min_raised = tightened(old_min, new_min, |n, o| n > o);
    let max_lowered = tightened(old_max, new_max, |n, o| n < o);
    let option_removed = old
        .options
        .iter()
        .any(|o| !new.options.iter().any(|n| n.value == o.value));
    min_raised
        || max_lowered
        || option_removed
        || new.group_max_items() < old.group_max_items()
        || group_fields_narrowed(old, new)
}

/// グループの中のフィールドが、外れる・型が変わる・必須になる（加わる）・値が狭まるか。
fn group_fields_narrowed(old: &Field, new: &Field) -> bool {
    old.fields.iter().any(|of| {
        new.inner(&of.name)
            .is_none_or(|nf| shape_changed(of, nf) || narrowed(of, nf))
    }) || new.fields.iter().any(|nf| {
        nf.required
            && old
                .inner(&nf.name)
                .is_none_or(|of| !of.required || shape_changed(of, nf))
    })
}

/// `old` から `new` への破壊的な変更の一覧。`old` がなければ（最初のデプロイ）空。
pub fn breaking_changes(old: Option<&Manifest>, new: &Manifest) -> Vec<Breaking> {
    let Some(old) = old else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let new_types = types_by_name(new);
    for (name, old_ct) in types_by_name(old) {
        let Some(new_ct) = new_types.get(name) else {
            out.push(Breaking::ContentTypeRemoved(name.to_owned()));
            continue;
        };
        for of in &old_ct.fields {
            match new_ct.field(&of.name) {
                None => out.push(Breaking::FieldRemoved {
                    content_type: name.to_owned(),
                    field: of.name.clone(),
                }),
                Some(nf) => {
                    if shape_changed(of, nf) {
                        out.push(Breaking::FieldTypeChanged {
                            content_type: name.to_owned(),
                            field: of.name.clone(),
                        });
                    } else {
                        if nf.required && !of.required {
                            out.push(Breaking::FieldBecameRequired {
                                content_type: name.to_owned(),
                                field: of.name.clone(),
                            });
                        }
                        if narrowed(of, nf) {
                            out.push(Breaking::FieldNarrowed {
                                content_type: name.to_owned(),
                                field: of.name.clone(),
                            });
                        }
                    }
                }
            }
        }
        for nf in &new_ct.fields {
            if nf.required && old_ct.field(&nf.name).is_none() {
                out.push(Breaking::RequiredFieldAdded {
                    content_type: name.to_owned(),
                    field: nf.name.clone(),
                });
            }
        }
    }
    let removed = |a: Vec<&String>, b: Vec<&String>| -> Vec<String> {
        let b: BTreeSet<&String> = b.into_iter().collect();
        a.into_iter().filter(|x| !b.contains(x)).cloned().collect()
    };
    for l in removed(old.locales.iter().collect(), new.locales.iter().collect()) {
        out.push(Breaking::LocaleRemoved(l));
    }
    let (old_nodes, new_nodes) = (old.body.all_nodes(), new.body.all_nodes());
    for n in removed(
        old_nodes.iter().map(|n| &n.name).collect(),
        new_nodes.iter().map(|n| &n.name).collect(),
    ) {
        out.push(Breaking::BodyNodeRemoved(n));
    }
    for n in removed(
        old.body.marks.iter().map(|n| &n.name).collect(),
        new.body.marks.iter().map(|n| &n.name).collect(),
    ) {
        out.push(Breaking::BodyMarkRemoved(n));
    }
    for h in removed(
        old.body.embed_providers.iter().collect(),
        new.body.embed_providers.iter().collect(),
    ) {
        out.push(Breaking::EmbedProviderRemoved(h));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::manifest::tests::sample;
    use crate::schema::manifest::{FieldType, Label};

    #[test]
    fn additions_are_not_breaking() {
        let old = sample();
        let mut new = sample();
        new.content_types[0].fields.push(Field {
            name: "added_field".into(),
            kind: FieldType::Text,
            required: false,
            reference_to: None,
            multiple: false,
            label: None,
            description: None,
            dev_note: None,
            searchable: false,
            ..Default::default()
        });
        new.locales.push("fr".into());
        new.content_types[0].description = Some("changed".into());
        new.content_types[0].label = Some(Label {
            ja: Some("記事".into()),
            en: None,
        });
        new.content_types[0].list_limit = 5;
        assert!(breaking_changes(Some(&old), &new).is_empty());
        assert!(breaking_changes(None, &new).is_empty());
    }

    fn field(v: serde_json::Value) -> Field {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn narrowing_is_found_and_widening_is_not() {
        use serde_json::{Value::Null, json};
        let num = |min: serde_json::Value, max: serde_json::Value| {
            let mut v = serde_json::json!({"name": "n", "type": "number"});
            if !min.is_null() {
                v["min"] = min;
            }
            if !max.is_null() {
                v["max"] = max;
            }
            field(v)
        };
        assert!(narrowed(
            &num(json!(0), json!(10)),
            &num(json!(1), json!(10))
        ));
        assert!(narrowed(
            &num(json!(0), json!(10)),
            &num(json!(0), json!(9.5))
        ));
        assert!(narrowed(&num(Null, Null), &num(json!(0), Null)));
        assert!(!narrowed(&num(json!(0), json!(10)), &num(json!(-1), Null)));

        let sel = |values: &[&str]| {
            field(json!({"name": "s", "type": "select",
                "options": values.iter().map(|v| json!({"value": v})).collect::<Vec<_>>()}))
        };
        assert!(narrowed(&sel(&["a", "b"]), &sel(&["a"])));
        assert!(!narrowed(&sel(&["a"]), &sel(&["a", "b"])));

        let group = |max: u32, fields: serde_json::Value| {
            field(json!({"name": "g", "type": "group", "maxItems": max, "fields": fields}))
        };
        let base = json!([{"name": "q", "type": "text"}, {"name": "n", "type": "number"}]);
        assert!(!narrowed(&group(5, base.clone()), &group(6, base.clone())));
        assert!(narrowed(&group(5, base.clone()), &group(4, base.clone())));
        // 中のフィールドを外す・型を変える・必須にする・必須で加える・値を狭める
        for changed in [
            json!([{"name": "q", "type": "text"}]),
            json!([{"name": "q", "type": "date"}, {"name": "n", "type": "number"}]),
            json!([{"name": "q", "type": "text", "required": true}, {"name": "n", "type": "number"}]),
            json!([{"name": "q", "type": "text"}, {"name": "n", "type": "number"}, {"name": "r", "type": "text", "required": true}]),
            json!([{"name": "q", "type": "text"}, {"name": "n", "type": "number", "max": 1}]),
        ] {
            assert!(
                narrowed(&group(5, base.clone()), &group(5, changed.clone())),
                "{changed}"
            );
        }
        // 任意のフィールドを加えるのは狭めない
        let added = json!([{"name": "q", "type": "text"}, {"name": "n", "type": "number"}, {"name": "r", "type": "text"}]);
        assert!(!narrowed(&group(5, base.clone()), &group(5, added)));

        // 整数かどうかの切り替えは、列の形の変更
        assert!(shape_changed(
            &num(Null, Null),
            &field(json!({"name": "n", "type": "number", "integer": true}))
        ));
    }

    #[test]
    fn narrowing_is_breaking() {
        let mut old = sample();
        old.content_types[0].fields.push(field(serde_json::json!(
            {"name": "s", "type": "select", "options": [{"value": "a"}, {"value": "b"}]}
        )));
        let mut new = old.clone();
        new.content_types[0].fields[4].options.pop();
        let found: Vec<String> = breaking_changes(Some(&old), &new)
            .iter()
            .map(Breaking::describe)
            .collect();
        assert_eq!(found, ["field `post.s` narrows the values it accepts"]);
    }

    #[test]
    fn breaking_changes_are_found() {
        let old = sample();
        let mut new = sample();
        new.content_types[0].fields.retain(|f| f.name != "body");
        new.content_types[0].fields[0].kind = FieldType::Richtext;
        new.content_types[0]
            .fields
            .iter_mut()
            .find(|f| f.name == "related")
            .unwrap()
            .required = true;
        new.content_types[0].fields.push(Field {
            name: "must".into(),
            kind: FieldType::Text,
            required: true,
            reference_to: None,
            multiple: false,
            label: None,
            description: None,
            dev_note: None,
            searchable: false,
            ..Default::default()
        });
        new.locales.retain(|l| l != "en");
        new.body.marks.retain(|m| m.name != "bold");
        new.body.embed_providers.clear();
        let found: Vec<String> = breaking_changes(Some(&old), &new)
            .iter()
            .map(Breaking::describe)
            .collect();
        for needle in [
            "`post.body` is removed",
            "`post.title` changes its type",
            "`post.related` becomes required",
            "required field `post.must` is added",
            "locale `en` is removed",
            "body mark `bold` is removed",
            "embed provider `www.youtube.com` is removed",
        ] {
            assert!(
                found.iter().any(|f| f.contains(needle)),
                "{needle}: {found:?}"
            );
        }
        let mut none = sample();
        none.content_types.clear();
        assert_eq!(
            breaking_changes(Some(&old), &none),
            vec![Breaking::ContentTypeRemoved("post".into())]
        );
    }
}
