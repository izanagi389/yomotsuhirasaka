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
    FieldRemoved { content_type: String, field: String },
    FieldTypeChanged { content_type: String, field: String },
    FieldBecameRequired { content_type: String, field: String },
    RequiredFieldAdded { content_type: String, field: String },
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

/// 列の形が変わるか（型・複数か・参照先）。変わるなら、列を作り直す（中身は消える）。
pub fn shape_changed(old: &Field, new: &Field) -> bool {
    old.kind != new.kind || old.multiple != new.multiple || old.reference_to != new.reference_to
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
                    } else if nf.required && !of.required {
                        out.push(Breaking::FieldBecameRequired {
                            content_type: name.to_owned(),
                            field: of.name.clone(),
                        });
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
    use crate::schema::manifest::FieldType;
    use crate::schema::manifest::tests::sample;

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
            description: None,
            dev_note: None,
        });
        new.locales.push("fr".into());
        new.content_types[0].description = Some("changed".into());
        new.content_types[0].list_limit = 5;
        assert!(breaking_changes(Some(&old), &new).is_empty());
        assert!(breaking_changes(None, &new).is_empty());
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
            description: None,
            dev_note: None,
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
