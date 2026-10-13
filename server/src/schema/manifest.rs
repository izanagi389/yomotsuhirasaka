//! テンプレートのマニフェスト（デプロイのAPIが受け取るJSON）と、その検査（02-schema-and-delivery-api.md、Q49, Q57, Q92, Q102）。
//!
//! CLIでも同じ検査を行うが、サーバーでも必ずやり直す（13-security.md「デプロイ」）。
//! ここを通った名前だけが、DBの識別子（`ct_<名前>`, `f_<名前>`）とGraphQLの名前の元になる。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// コンテンツタイプ名・フィールド名の最大の長さ（DBの識別子の上限63に、接頭辞を足しても収まる長さ）。
pub const MAX_NAME_LEN: usize = 40;
/// テンプレート名の最大の長さ。
pub const MAX_TEMPLATE_NAME_LEN: usize = 64;
const MAX_CONTENT_TYPES: usize = 100;
const MAX_FIELDS: usize = 200;
const MAX_LOCALES: usize = 50;
const MAX_BODY_NODES: usize = 100;
const MAX_TEXT_CHARS: usize = 2000;
/// 一覧のフィールドで1回に取得できる件数の上限の、さらに上限。
pub const MAX_LIST_LIMIT: u32 = 1000;

/// フィールドの型（M1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    /// 1行の文字列。
    Text,
    /// URLに使う文字列（英小文字・数字・ハイフン）。
    Slug,
    /// 本文（Tiptapの構造化JSON）。
    Richtext,
    /// 他の記事への参照（翻訳グループを指す。Q157）。
    Reference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: FieldType,
    #[serde(default)]
    pub required: bool,
    /// 参照フィールドの参照先のコンテンツタイプ。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_to: Option<String>,
    /// 複数の値を持つか（参照フィールドだけ）。
    #[serde(default)]
    pub multiple: bool,
    /// 編集者向けの説明（Q51）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 開発者向けのメモ。どのAPIにも出さない（Q51、INV-SCH-01）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContentType {
    pub name: String,
    /// ロケールごとの公開URLのテンプレート。使える差し込みは `{locale}` と `{slug}`（Q82）。
    pub url_template: String,
    /// 一覧のフィールドで1回に取得できる件数の上限。
    pub list_limit: u32,
    pub fields: Vec<Field>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev_note: Option<String>,
}

/// 本文のノードの定義（Tiptapの拡張機能から、CLIが取り出したもの。Q57, Q142）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BodyNode {
    pub name: String,
    /// 子として許すノードの名前（content式を簡略化したもの）。
    #[serde(default)]
    pub children: Vec<String>,
    /// 許す属性の名前。
    #[serde(default)]
    pub attrs: Vec<String>,
    /// 子のテキストに書式（マーク）を付けられるか。
    #[serde(default)]
    pub marks: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BodyMark {
    pub name: String,
    #[serde(default)]
    pub attrs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Body {
    pub nodes: Vec<BodyNode>,
    #[serde(default)]
    pub marks: Vec<BodyMark>,
    /// 埋め込みを許す提供元のホスト名。
    #[serde(default)]
    pub embed_providers: Vec<String>,
}

/// テンプレートのマニフェスト。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub locales: Vec<String>,
    pub default_locale: String,
    pub body: Body,
    pub content_types: Vec<ContentType>,
}

impl Manifest {
    pub fn content_type(&self, name: &str) -> Option<&ContentType> {
        self.content_types.iter().find(|c| c.name == name)
    }

    /// `devNote` を除いた写し（スキーマ情報のAPIで返す。INV-SCH-01）。
    pub fn without_dev_notes(&self) -> Self {
        let mut m = self.clone();
        for ct in &mut m.content_types {
            ct.dev_note = None;
            for f in &mut ct.fields {
                f.dev_note = None;
            }
        }
        m
    }
}

impl ContentType {
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }
}

/// コンテンツタイプ名・フィールド名として正しいか: 英小文字で始まり、英小文字・数字・アンダースコアだけ（Q92）。
pub fn valid_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z'))
        && name.len() <= MAX_NAME_LEN
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// テンプレート名として正しいか: 英小文字・数字・ハイフン（ワークスペースの作成と同じ規則）。
pub fn valid_template_name(name: &str) -> bool {
    (1..=MAX_TEMPLATE_NAME_LEN).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// 本文のノード・マークの名前（Tiptapの名前。英字で始まる英数字）。
fn valid_node_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z' | b'A'..=b'Z'))
        && name.len() <= MAX_NAME_LEN
        && bytes.all(|b| b.is_ascii_alphanumeric())
}

/// ロケールのコード（例: `ja`, `en`, `pt-BR`, `zh-Hant`）。
fn valid_locale(code: &str) -> bool {
    let mut parts = code.split('-');
    let Some(lang) = parts.next() else {
        return false;
    };
    (2..=3).contains(&lang.len())
        && lang.bytes().all(|b| b.is_ascii_lowercase())
        && parts.all(|p| (2..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// ホスト名（埋め込みの提供元）。
fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        && host.contains('.')
}

/// URLのテンプレート: `https://` か `http://` で始まり、差し込みは `{locale}` と `{slug}` だけ。`{slug}` は必須。
fn valid_url_template(t: &str) -> bool {
    let Some(rest) = t
        .strip_prefix("https://")
        .or_else(|| t.strip_prefix("http://"))
    else {
        return false;
    };
    if rest.is_empty()
        || t.len() > MAX_TEXT_CHARS
        || t.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return false;
    }
    let mut placeholders = Vec::new();
    let mut s = rest;
    while let Some(start) = s.find('{') {
        let Some(end) = s[start..].find('}') else {
            return false;
        };
        placeholders.push(&s[start + 1..start + end]);
        s = &s[start + end + 1..];
    }
    if s.contains('}') {
        return false;
    }
    placeholders.iter().all(|p| *p == "locale" || *p == "slug") && placeholders.contains(&"slug")
}

fn check_text(problems: &mut Vec<String>, path: &str, value: &Option<String>) {
    if let Some(v) = value
        && v.chars().count() > MAX_TEXT_CHARS
    {
        problems.push(format!(
            "{path}: must be at most {MAX_TEXT_CHARS} characters"
        ));
    }
}

/// マニフェストを検査し、問題の一覧を返す（空なら正しい）。メッセージには場所と理由だけを書く。
pub fn validate(m: &Manifest) -> Vec<String> {
    let mut problems = Vec::new();

    // ロケール
    if m.locales.is_empty() || m.locales.len() > MAX_LOCALES {
        problems.push(format!("locales: must have 1 to {MAX_LOCALES} entries"));
    }
    let mut seen = BTreeSet::new();
    for (i, l) in m.locales.iter().enumerate() {
        if !valid_locale(l) {
            problems.push(format!("locales[{i}]: invalid locale code"));
        }
        if !seen.insert(l) {
            problems.push(format!("locales[{i}]: duplicated"));
        }
    }
    if !m.locales.contains(&m.default_locale) {
        problems.push("defaultLocale: must be one of locales".to_owned());
    }

    // 本文のノードとマーク
    let body = &m.body;
    if body.nodes.len() > MAX_BODY_NODES || body.marks.len() > MAX_BODY_NODES {
        problems.push(format!("body: at most {MAX_BODY_NODES} nodes and marks"));
    }
    let mut node_names = BTreeSet::new();
    for (i, n) in body.nodes.iter().enumerate() {
        if !valid_node_name(&n.name) {
            problems.push(format!("body.nodes[{i}].name: invalid node name"));
        }
        if !node_names.insert(n.name.as_str()) {
            problems.push(format!("body.nodes[{i}].name: duplicated"));
        }
        for (j, a) in n.attrs.iter().enumerate() {
            if !valid_node_name(a) {
                problems.push(format!(
                    "body.nodes[{i}].attrs[{j}]: invalid attribute name"
                ));
            }
        }
    }
    for required in ["doc", "text"] {
        if !node_names.contains(required) {
            problems.push(format!("body.nodes: `{required}` is required"));
        }
    }
    // CMSが意味を知っているノードは、検証と公開の検査に使う属性を持たなければならない（Q75, Q97）
    for (i, n) in body.nodes.iter().enumerate() {
        let needed: &[&str] = match n.name.as_str() {
            "image" => &["mediaId", "alt", "decorative"],
            "embed" => &["src"],
            _ => &[],
        };
        for a in needed {
            if !n.attrs.iter().any(|x| x == a) {
                problems.push(format!(
                    "body.nodes[{i}].attrs: `{}` needs the attribute `{a}`",
                    n.name
                ));
            }
        }
    }
    for (i, n) in body.nodes.iter().enumerate() {
        for (j, c) in n.children.iter().enumerate() {
            if !node_names.contains(c.as_str()) {
                problems.push(format!("body.nodes[{i}].children[{j}]: unknown node"));
            }
        }
    }
    let mut mark_names = BTreeSet::new();
    for (i, mk) in body.marks.iter().enumerate() {
        if !valid_node_name(&mk.name) {
            problems.push(format!("body.marks[{i}].name: invalid mark name"));
        }
        if !mark_names.insert(mk.name.as_str()) {
            problems.push(format!("body.marks[{i}].name: duplicated"));
        }
        for (j, a) in mk.attrs.iter().enumerate() {
            if !valid_node_name(a) {
                problems.push(format!(
                    "body.marks[{i}].attrs[{j}]: invalid attribute name"
                ));
            }
        }
    }
    let mut providers = BTreeSet::new();
    for (i, h) in body.embed_providers.iter().enumerate() {
        if !valid_host(h) {
            problems.push(format!("body.embedProviders[{i}]: invalid host name"));
        }
        if !providers.insert(h) {
            problems.push(format!("body.embedProviders[{i}]: duplicated"));
        }
    }

    // コンテンツタイプとフィールド
    if m.content_types.len() > MAX_CONTENT_TYPES {
        problems.push(format!("contentTypes: at most {MAX_CONTENT_TYPES}"));
    }
    let type_names: BTreeSet<&str> = m.content_types.iter().map(|c| c.name.as_str()).collect();
    let mut seen_types = BTreeSet::new();
    for (i, ct) in m.content_types.iter().enumerate() {
        let path = format!("contentTypes[{i}]");
        if !valid_name(&ct.name) {
            problems.push(format!(
                "{path}.name: use 1 to {MAX_NAME_LEN} characters of a-z, 0-9 and _, starting with a letter"
            ));
        }
        if !seen_types.insert(ct.name.as_str()) {
            problems.push(format!("{path}.name: duplicated"));
        }
        if !valid_url_template(&ct.url_template) {
            problems.push(format!(
                "{path}.urlTemplate: must be an http(s) URL using only {{locale}} and {{slug}}, and include {{slug}}"
            ));
        }
        if !(1..=MAX_LIST_LIMIT).contains(&ct.list_limit) {
            problems.push(format!("{path}.listLimit: must be 1 to {MAX_LIST_LIMIT}"));
        }
        check_text(
            &mut problems,
            &format!("{path}.description"),
            &ct.description,
        );
        check_text(&mut problems, &format!("{path}.devNote"), &ct.dev_note);
        if ct.fields.len() > MAX_FIELDS {
            problems.push(format!("{path}.fields: at most {MAX_FIELDS}"));
        }
        let mut seen_fields = BTreeSet::new();
        for (j, f) in ct.fields.iter().enumerate() {
            let fpath = format!("{path}.fields[{j}]");
            if !valid_name(&f.name) {
                problems.push(format!(
                    "{fpath}.name: use 1 to {MAX_NAME_LEN} characters of a-z, 0-9 and _, starting with a letter"
                ));
            }
            if !seen_fields.insert(f.name.as_str()) {
                problems.push(format!("{fpath}.name: duplicated"));
            }
            match (f.kind, &f.reference_to) {
                (FieldType::Reference, Some(to)) if type_names.contains(to.as_str()) => {}
                (FieldType::Reference, _) => {
                    problems.push(format!("{fpath}.referenceTo: must name a content type"))
                }
                (_, Some(_)) => problems.push(format!(
                    "{fpath}.referenceTo: only reference fields can have it"
                )),
                (_, None) => {}
            }
            if f.multiple && f.kind != FieldType::Reference {
                problems.push(format!(
                    "{fpath}.multiple: only reference fields can be multiple"
                ));
            }
            check_text(
                &mut problems,
                &format!("{fpath}.description"),
                &f.description,
            );
            check_text(&mut problems, &format!("{fpath}.devNote"), &f.dev_note);
        }
        // URLを作るためのスラッグ
        let slugs = ct
            .fields
            .iter()
            .filter(|f| f.kind == FieldType::Slug)
            .count();
        if slugs != 1 {
            problems.push(format!("{path}.fields: exactly one slug field is required"));
        }
    }
    problems
}

/// 名前 → 定義の対応（差分を取るため）。
pub fn types_by_name(m: &Manifest) -> BTreeMap<&str, &ContentType> {
    m.content_types
        .iter()
        .map(|c| (c.name.as_str(), c))
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn sample() -> Manifest {
        serde_json::from_str(
            r#"{
              "locales": ["ja", "en"],
              "defaultLocale": "ja",
              "body": {
                "nodes": [
                  {"name": "doc", "children": ["paragraph"]},
                  {"name": "paragraph", "children": ["text"], "marks": true},
                  {"name": "text"}
                ],
                "marks": [{"name": "bold"}, {"name": "link", "attrs": ["href"]}],
                "embedProviders": ["www.youtube.com"]
              },
              "contentTypes": [{
                "name": "post",
                "urlTemplate": "https://example.com/{locale}/blog/{slug}",
                "listLimit": 20,
                "devNote": "secret note",
                "fields": [
                  {"name": "title", "type": "text", "required": true},
                  {"name": "slug", "type": "slug", "required": true},
                  {"name": "body", "type": "richtext"},
                  {"name": "related", "type": "reference", "referenceTo": "post", "multiple": true, "devNote": "x"}
                ]
              }]
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn sample_is_valid() {
        assert_eq!(validate(&sample()), Vec::<String>::new());
    }

    #[test]
    fn known_nodes_need_their_attributes() {
        let mut m = sample();
        m.body.nodes[0].children.push("image".into());
        m.body
            .nodes
            .push(serde_json::from_str(r#"{"name": "image", "attrs": ["mediaId"]}"#).unwrap());
        let problems = validate(&m);
        assert!(problems.iter().any(|p| p.contains("`alt`")), "{problems:?}");
        assert!(
            problems.iter().any(|p| p.contains("`decorative`")),
            "{problems:?}"
        );
        m.body.nodes.last_mut().unwrap().attrs =
            vec!["mediaId".into(), "alt".into(), "decorative".into()];
        assert_eq!(validate(&m), Vec::<String>::new());
    }

    #[test]
    fn names() {
        for ok in ["post", "valid_name_2", "a", &"a".repeat(MAX_NAME_LEN)] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in [
            "",
            "Post",
            "post-name",
            "post name",
            "post\";drop table x;--",
            "ポスト",
            "post$",
            "post.x",
            "2post",
            "_post",
            &"a".repeat(MAX_NAME_LEN + 1),
            "addedField",
        ] {
            assert!(!valid_name(bad), "{bad}");
        }
        assert!(valid_template_name("inv-sch02-1a2b3c4d"));
        assert!(!valid_template_name("Inv"));
        assert!(
            valid_locale("pt-BR")
                && valid_locale("zh-Hant")
                && !valid_locale("JA")
                && !valid_locale("j")
        );
        assert!(valid_host("www.youtube.com") && !valid_host("localhost") && !valid_host("a..b"));
    }

    #[test]
    fn url_templates() {
        assert!(valid_url_template("https://example.com/{locale}/{slug}"));
        assert!(valid_url_template("http://localhost:3000/{slug}"));
        for bad in [
            "https://example.com/{locale}",
            "https://example.com/{id}/{slug}",
            "javascript:alert(1)//{slug}",
            "/blog/{slug}",
            "https://example.com/{slug",
            "https://example.com/slug}",
            "https://exa mple.com/{slug}",
        ] {
            assert!(!valid_url_template(bad), "{bad}");
        }
    }

    #[test]
    fn invalid_manifests_are_reported() {
        let mut m = sample();
        m.content_types[0].name = "Post".into();
        m.content_types[0].fields.push(Field {
            name: "post-name".into(),
            kind: FieldType::Text,
            required: false,
            reference_to: Some("post".into()),
            multiple: true,
            description: None,
            dev_note: None,
        });
        m.default_locale = "fr".into();
        m.body.nodes.retain(|n| n.name != "text");
        let problems = validate(&m);
        for needle in [
            "contentTypes[0].name",
            "fields[4].name",
            "fields[4].referenceTo",
            "fields[4].multiple",
            "defaultLocale",
            "`text` is required",
            "children[0]: unknown node",
            // 参照先も Post に変わったため
            "fields[3].referenceTo",
        ] {
            assert!(
                problems.iter().any(|p| p.contains(needle)),
                "{needle}: {problems:?}"
            );
        }
    }

    #[test]
    fn dev_notes_are_removed() {
        let m = sample().without_dev_notes();
        assert!(!serde_json::to_string(&m).unwrap().contains("secret note"));
        assert!(!serde_json::to_string(&m).unwrap().contains("devNote"));
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let mut v = serde_json::to_value(sample()).unwrap();
        v["contentTypes"][0]["fields"][0]["sql"] = "x".into();
        assert!(serde_json::from_value::<Manifest>(v).is_err());
    }
}
