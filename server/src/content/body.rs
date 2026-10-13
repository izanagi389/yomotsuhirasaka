//! 本文（Tiptapの構造化JSON）の検証とアンカーIDの付与（03-content-and-editing.md「本文」、13-security.md「本文」）。
//!
//! - マニフェストのノード定義に従い、定義にないノード・属性・マーク、許可されていない子ノードを含む本文は拒否する（Q56, Q57, Q142）。
//! - リンクの `href` は `https`・`http`・`mailto`・`tel`・相対パス・`#` で始まるアンカーだけ（Q97）。
//! - 埋め込み（`embed`）は、許可した提供元の https のURLだけ（Q97）。
//! - 見出しには、保存のたびにサーバーがアンカーIDを最終的に決める。IDのない見出しに付け、同じ本文の中で重複していれば
//!   2つ目以降に新しいIDを振る（Q28, Q136）。
//! - 問題の説明には、場所（JSONのパス）と理由だけを書き、本文の内容は書かない（ログや応答に本文を出さないため）。

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value};

use crate::schema::manifest::Body;

/// 見出しのノードの名前と、アンカーIDの属性の名前。アンカーIDは、マニフェストの定義によらず見出しに付ける。
pub const HEADING: &str = "heading";
pub const ANCHOR_ATTR: &str = "id";
/// CMSが意味を知っているノード。
const EMBED: &str = "embed";
pub const IMAGE: &str = "image";
const LINK: &str = "link";

const MAX_DEPTH: usize = 50;
const MAX_NODES: usize = 50_000;
const MAX_TEXT_CHARS: usize = 100_000;
const MAX_ATTR_CHARS: usize = 2_000;
const MAX_ANCHOR_LEN: usize = 64;

/// リンクに使えるスキーム（小文字で比べる）。
const LINK_SCHEMES: [&str; 4] = ["https", "http", "mailto", "tel"];

struct Ctx<'a> {
    body: &'a Body,
    nodes: HashMap<&'a str, &'a crate::schema::manifest::BodyNode>,
    marks: HashMap<&'a str, &'a crate::schema::manifest::BodyMark>,
    problems: Vec<String>,
    count: usize,
    /// 出てきた順の見出しのパス（アンカーIDを付けるため）。
    headings: Vec<Vec<usize>>,
}

impl Ctx<'_> {
    fn problem(&mut self, path: &str, reason: &str) {
        if self.problems.len() < 50 {
            self.problems.push(format!("{path}: {reason}"));
        }
    }
}

fn is_scalar(v: &Value) -> bool {
    matches!(
        v,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    )
}

fn has_control(s: &str) -> bool {
    s.chars().any(|c| c.is_control())
}

/// リンクの `href` として許すか。
pub fn valid_href(href: &str) -> bool {
    if href.is_empty()
        || href.len() > MAX_ATTR_CHARS
        || href.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return false;
    }
    // 相対パスとアンカー（スキームを持たない形だけ）
    if href.starts_with('#')
        || href.starts_with('?')
        || href.starts_with("./")
        || href.starts_with("../")
    {
        return true;
    }
    if let Some(rest) = href.strip_prefix('/') {
        // `//host`（スキームを省いた別のホスト）と `/\host` は相対パスとして扱わない
        return !rest.starts_with('/') && !rest.starts_with('\\');
    }
    // スキームのある絶対URL: 元の文字列の先頭が「英字のスキーム + :」で、許可したスキームであること
    let Some((scheme, _)) = href.split_once(':') else {
        return false;
    };
    if scheme.is_empty() || !scheme.bytes().all(|b| b.is_ascii_alphabetic()) {
        return false;
    }
    let scheme = scheme.to_ascii_lowercase();
    if !LINK_SCHEMES.contains(&scheme.as_str()) {
        return false;
    }
    match url::Url::parse(href) {
        Ok(u) => {
            u.scheme() == scheme
                && (!matches!(scheme.as_str(), "http" | "https") || u.host().is_some())
        }
        Err(_) => false,
    }
}

/// 埋め込みのURLとして許すか: https で、ホストが許可した提供元と完全に一致し、ユーザー情報やポートの指定がないこと。
pub fn valid_embed(src: &str, providers: &[String]) -> bool {
    if src.len() > MAX_ATTR_CHARS || src.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let Ok(u) = url::Url::parse(src) else {
        return false;
    };
    u.scheme() == "https"
        && u.username().is_empty()
        && u.password().is_none()
        && u.port().is_none()
        && u.host_str()
            .is_some_and(|h| providers.iter().any(|p| p.eq_ignore_ascii_case(h)))
}

fn valid_anchor(id: &str) -> bool {
    (1..=MAX_ANCHOR_LEN).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn new_anchor() -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut bytes = [0u8; 10];
    rand::fill(&mut bytes);
    let s: String = bytes
        .iter()
        .map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char)
        .collect();
    format!("h-{s}")
}

fn check_attrs(
    ctx: &mut Ctx,
    path: &str,
    attrs: Option<&Value>,
    allowed: &[String],
    extra: &[&str],
) {
    let Some(attrs) = attrs else {
        return;
    };
    let Some(obj) = attrs.as_object() else {
        ctx.problem(path, "attrs must be an object");
        return;
    };
    for (k, v) in obj {
        if !allowed.iter().any(|a| a == k) && !extra.contains(&k.as_str()) {
            ctx.problem(&format!("{path}.attrs.{k}"), "attribute is not allowed");
            continue;
        }
        if !is_scalar(v) {
            ctx.problem(
                &format!("{path}.attrs.{k}"),
                "attribute must be a string, number, boolean or null",
            );
        } else if let Value::String(s) = v
            && (s.chars().count() > MAX_ATTR_CHARS || has_control(s))
        {
            ctx.problem(
                &format!("{path}.attrs.{k}"),
                "attribute value is too long or contains control characters",
            );
        }
    }
}

fn check_marks(ctx: &mut Ctx, path: &str, marks: &Value, parent_allows_marks: bool) {
    let Some(list) = marks.as_array() else {
        ctx.problem(path, "marks must be an array");
        return;
    };
    if !parent_allows_marks && !list.is_empty() {
        ctx.problem(path, "marks are not allowed here");
        return;
    }
    for (i, m) in list.iter().enumerate() {
        let mpath = format!("{path}[{i}]");
        let Some(obj) = m.as_object() else {
            ctx.problem(&mpath, "mark must be an object");
            continue;
        };
        if let Some(k) = obj.keys().find(|k| *k != "type" && *k != "attrs") {
            ctx.problem(&format!("{mpath}.{k}"), "unknown key");
        }
        let Some(ty) = obj.get("type").and_then(Value::as_str) else {
            ctx.problem(&mpath, "mark type is required");
            continue;
        };
        let Some(def) = ctx.marks.get(ty).copied() else {
            ctx.problem(&mpath, "mark is not defined in the template");
            continue;
        };
        check_attrs(ctx, &mpath, obj.get("attrs"), &def.attrs, &[]);
        if ty == LINK {
            let href = obj
                .get("attrs")
                .and_then(|a| a.get("href"))
                .and_then(Value::as_str);
            if !href.is_some_and(valid_href) {
                ctx.problem(
                    &format!("{mpath}.attrs.href"),
                    "link must use https, http, mailto, tel, a relative path or #",
                );
            }
        }
    }
}

fn check_node(
    ctx: &mut Ctx,
    node: &Value,
    path: &str,
    index_path: &mut Vec<usize>,
    allowed_children: Option<&[String]>,
    parent_allows_marks: bool,
) {
    ctx.count += 1;
    if ctx.count > MAX_NODES {
        if ctx.count == MAX_NODES + 1 {
            ctx.problem(path, "the body has too many nodes");
        }
        return;
    }
    if index_path.len() > MAX_DEPTH {
        ctx.problem(path, "the body is nested too deeply");
        return;
    }
    let Some(obj) = node.as_object() else {
        ctx.problem(path, "node must be an object");
        return;
    };
    if let Some(k) = obj
        .keys()
        .find(|k| !matches!(k.as_str(), "type" | "attrs" | "content" | "marks" | "text"))
    {
        ctx.problem(&format!("{path}.{k}"), "unknown key");
    }
    let Some(ty) = obj.get("type").and_then(Value::as_str) else {
        ctx.problem(path, "node type is required");
        return;
    };
    let Some(def) = ctx.nodes.get(ty).copied() else {
        ctx.problem(path, "node is not defined in the template");
        return;
    };
    if let Some(allowed) = allowed_children
        && !allowed.iter().any(|c| c == ty)
    {
        ctx.problem(path, "node is not allowed here");
        return;
    }

    if ty == "text" {
        match obj.get("text").and_then(Value::as_str) {
            Some(t) if !t.is_empty() && t.chars().count() <= MAX_TEXT_CHARS => {}
            _ => ctx.problem(path, "text node needs non-empty text"),
        }
        if obj.contains_key("content") || obj.contains_key("attrs") {
            ctx.problem(path, "text node cannot have content or attrs");
        }
        if let Some(marks) = obj.get("marks") {
            check_marks(ctx, &format!("{path}.marks"), marks, parent_allows_marks);
        }
        return;
    }
    if obj.contains_key("text") {
        ctx.problem(&format!("{path}.text"), "only text nodes can have text");
    }
    if obj.contains_key("marks") {
        ctx.problem(&format!("{path}.marks"), "only text nodes can have marks");
    }
    let extra: &[&str] = if ty == HEADING { &[ANCHOR_ATTR] } else { &[] };
    check_attrs(ctx, path, obj.get("attrs"), &def.attrs, extra);
    if ty == HEADING {
        ctx.headings.push(index_path.clone());
    }
    if ty == EMBED {
        let src = obj
            .get("attrs")
            .and_then(|a| a.get("src"))
            .and_then(Value::as_str);
        if !src.is_some_and(|s| valid_embed(s, &ctx.body.embed_providers)) {
            ctx.problem(
                &format!("{path}.attrs.src"),
                "embed must be an https URL of an allowed provider",
            );
        }
    }
    if ty == IMAGE {
        let media = obj
            .get("attrs")
            .and_then(|a| a.get("mediaId"))
            .and_then(Value::as_str);
        if !media.is_some_and(|m| uuid::Uuid::parse_str(m).is_ok()) {
            ctx.problem(&format!("{path}.attrs.mediaId"), "image needs a media ID");
        }
    }
    match obj.get("content") {
        None => {}
        Some(Value::Array(children)) => {
            for (i, child) in children.iter().enumerate() {
                index_path.push(i);
                check_node(
                    ctx,
                    child,
                    &format!("{path}.content[{i}]"),
                    index_path,
                    Some(&def.children),
                    def.marks,
                );
                index_path.pop();
            }
        }
        Some(_) => ctx.problem(&format!("{path}.content"), "content must be an array"),
    }
}

/// 本文を検証し、アンカーIDを決めた本文を返す。問題があれば、その一覧を返す。
pub fn validate(doc: &Value, body: &Body) -> Result<Value, Vec<String>> {
    let mut ctx = Ctx {
        body,
        nodes: body.nodes.iter().map(|n| (n.name.as_str(), n)).collect(),
        marks: body.marks.iter().map(|m| (m.name.as_str(), m)).collect(),
        problems: Vec::new(),
        count: 0,
        headings: Vec::new(),
    };
    if doc.get("type").and_then(Value::as_str) != Some("doc") {
        return Err(vec!["$: the body must be a `doc` node".to_owned()]);
    }
    check_node(&mut ctx, doc, "$", &mut Vec::new(), None, false);
    if !ctx.problems.is_empty() {
        return Err(ctx.problems);
    }
    let mut out = doc.clone();
    assign_anchors(&mut out, &ctx.headings);
    normalize_media_ids(&mut out);
    Ok(out)
}

/// 画像のメディアのIDを、小文字・ハイフン区切りの形にそろえる（メディアの使用箇所を本文から探せるように）。
fn normalize_media_ids(v: &mut Value) {
    if v.get("type").and_then(Value::as_str) == Some(IMAGE)
        && let Some(attrs) = v.get_mut("attrs").and_then(Value::as_object_mut)
        && let Some(id) = attrs
            .get("mediaId")
            .and_then(Value::as_str)
            .and_then(|s| uuid::Uuid::parse_str(s).ok())
    {
        attrs.insert(
            "mediaId".to_owned(),
            Value::String(id.hyphenated().to_string()),
        );
    }
    if let Some(children) = v.get_mut("content").and_then(Value::as_array_mut) {
        for c in children {
            normalize_media_ids(c);
        }
    }
}

fn node_at<'a>(doc: &'a mut Value, path: &[usize]) -> Option<&'a mut Value> {
    let mut cur = doc;
    for &i in path {
        cur = cur.get_mut("content")?.get_mut(i)?;
    }
    Some(cur)
}

/// 見出しにアンカーIDを付ける。正しい形のIDは、本文の中で最初に出てきたものだけを残す。
fn assign_anchors(doc: &mut Value, headings: &[Vec<usize>]) {
    let mut seen: HashSet<String> = HashSet::new();
    for path in headings {
        let Some(node) = node_at(doc, path) else {
            continue;
        };
        let current = node
            .get("attrs")
            .and_then(|a| a.get(ANCHOR_ATTR))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let id = match current {
            Some(id) if valid_anchor(&id) && !seen.contains(&id) => id,
            _ => loop {
                let candidate = new_anchor();
                if !seen.contains(&candidate) {
                    break candidate;
                }
            },
        };
        seen.insert(id.clone());
        let obj = node.as_object_mut().expect("validated node is an object");
        let attrs = obj
            .entry("attrs")
            .or_insert_with(|| Value::Object(Map::new()));
        if !attrs.is_object() {
            *attrs = Value::Object(Map::new());
        }
        attrs
            .as_object_mut()
            .expect("attrs is an object")
            .insert(ANCHOR_ATTR.to_owned(), Value::String(id));
    }
}

/// 本文の中で使われているメディアのID（画像）。
pub fn media_ids(doc: &Value) -> Vec<uuid::Uuid> {
    let mut out = Vec::new();
    fn walk(v: &Value, out: &mut Vec<uuid::Uuid>) {
        if v.get("type").and_then(Value::as_str) == Some(IMAGE)
            && let Some(id) = v
                .get("attrs")
                .and_then(|a| a.get("mediaId"))
                .and_then(Value::as_str)
                .and_then(|s| uuid::Uuid::parse_str(s).ok())
        {
            out.push(id);
        }
        if let Some(children) = v.get("content").and_then(Value::as_array) {
            for c in children {
                walk(c, out);
            }
        }
    }
    walk(doc, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn media_ids_are_normalized() {
        let doc = json!({"type": "doc", "content": [{"type": "image", "attrs": {"mediaId": "0123456789ABCDEF0123456789ABCDEF", "alt": "a"}}]});
        let out = validate(&doc, &body()).unwrap();
        assert_eq!(
            out["content"][0]["attrs"]["mediaId"],
            "01234567-89ab-cdef-0123-456789abcdef"
        );
    }

    fn body() -> Body {
        serde_json::from_value(json!({
            "nodes": [
                {"name": "doc", "children": ["paragraph", "heading", "bulletList", "image", "embed"]},
                {"name": "paragraph", "children": ["text"], "marks": true},
                {"name": "heading", "children": ["text"], "attrs": ["level"], "marks": true},
                {"name": "bulletList", "children": ["listItem"]},
                {"name": "listItem", "children": ["paragraph", "bulletList"]},
                {"name": "text"},
                {"name": "image", "attrs": ["mediaId", "alt", "decorative"]},
                {"name": "embed", "attrs": ["src"]}
            ],
            "marks": [{"name": "bold"}, {"name": "link", "attrs": ["href", "target", "rel"]}],
            "embedProviders": ["www.youtube.com"]
        }))
        .unwrap()
    }

    fn p(text: &str) -> Value {
        json!({"type": "paragraph", "content": [{"type": "text", "text": text}]})
    }

    fn h(text: &str, id: Option<&str>) -> Value {
        match id {
            Some(id) => {
                json!({"type": "heading", "attrs": {"level": 2, "id": id}, "content": [{"type": "text", "text": text}]})
            }
            None => {
                json!({"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": text}]})
            }
        }
    }

    fn doc(nodes: Vec<Value>) -> Value {
        json!({"type": "doc", "content": nodes})
    }

    fn anchors(v: &Value) -> Vec<String> {
        v["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| n["type"] == "heading")
            .map(|n| n["attrs"]["id"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn accepts_valid_bodies() {
        let d = doc(vec![
            p("a"),
            json!({"type": "paragraph", "content": [{"type": "text", "text": "x", "marks": [{"type": "bold"}, {"type": "link", "attrs": {"href": "https://example.com"}}]}]}),
            json!({"type": "bulletList", "content": [{"type": "listItem", "content": [p("i")]}]}),
            json!({"type": "embed", "attrs": {"src": "https://www.youtube.com/embed/aaaaaaaaaaa"}}),
            json!({"type": "image", "attrs": {"mediaId": "0123456789abcdef0123456789abcdef", "alt": "a"}}),
            json!({"type": "paragraph"}),
        ]);
        assert!(
            validate(&d, &body()).is_ok(),
            "{:?}",
            validate(&d, &body()).err()
        );
    }

    #[test]
    fn rejects_undefined_nodes_attrs_marks_and_children() {
        for (label, bad) in [
            (
                "script",
                json!({"type": "script", "content": [{"type": "text", "text": "x"}]}),
            ),
            (
                "rawHtml",
                json!({"type": "rawHtml", "attrs": {"html": "<img>"}}),
            ),
            (
                "onclick",
                json!({"type": "paragraph", "attrs": {"onclick": "x"}, "content": [{"type": "text", "text": "x"}]}),
            ),
            (
                "style",
                json!({"type": "heading", "attrs": {"level": 2, "style": "x"}, "content": [{"type": "text", "text": "x"}]}),
            ),
            (
                "strike",
                json!({"type": "paragraph", "content": [{"type": "text", "text": "x", "marks": [{"type": "strike"}]}]}),
            ),
            (
                "mark attr",
                json!({"type": "paragraph", "content": [{"type": "text", "text": "x", "marks": [{"type": "link", "attrs": {"href": "https://e.com", "onmouseover": "x"}}]}]}),
            ),
            ("text in doc", json!({"type": "text", "text": "x"})),
            ("p in p", json!({"type": "paragraph", "content": [p("x")]})),
            (
                "list in heading",
                json!({"type": "heading", "attrs": {"level": 2}, "content": [{"type": "bulletList", "content": []}]}),
            ),
            (
                "p in list",
                json!({"type": "bulletList", "content": [p("x")]}),
            ),
            (
                "nested attr object",
                json!({"type": "heading", "attrs": {"level": {"x": 1}}, "content": [{"type": "text", "text": "x"}]}),
            ),
            ("unknown key", json!({"type": "paragraph", "html": "<b>"})),
            (
                "empty text",
                json!({"type": "paragraph", "content": [{"type": "text", "text": ""}]}),
            ),
            (
                "marks on block",
                json!({"type": "paragraph", "marks": [{"type": "bold"}]}),
            ),
            (
                "image without media",
                json!({"type": "image", "attrs": {"alt": "a"}}),
            ),
        ] {
            assert!(validate(&doc(vec![bad]), &body()).is_err(), "{label}");
        }
        assert!(validate(&json!({"type": "paragraph"}), &body()).is_err());
        assert!(validate(&json!("doc"), &body()).is_err());
    }

    #[test]
    fn link_schemes() {
        for ok in [
            "https://example.com/a",
            "http://example.com/a",
            "mailto:a@example.com",
            "tel:+81-3-0000-0000",
            "/relative/path",
            "#section",
            "./a",
            "../a",
            "?q=1",
            "HTTPS://EXAMPLE.COM",
        ] {
            assert!(valid_href(ok), "{ok}");
        }
        for bad in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            " javascript:alert(1)",
            "java\tscript:alert(1)",
            "javascript&colon;alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "vbscript:msgbox(1)",
            "file:///etc/passwd",
            "//evil.example",
            "/\\evil.example",
            "",
            "relative/path",
            "https:",
        ] {
            assert!(!valid_href(bad), "{bad:?}");
        }
    }

    #[test]
    fn embed_providers() {
        let providers = vec!["www.youtube.com".to_owned()];
        assert!(valid_embed(
            "https://www.youtube.com/embed/aaaaaaaaaaa",
            &providers
        ));
        assert!(valid_embed("https://WWW.YOUTUBE.COM/embed/x", &providers));
        for bad in [
            "https://evil.inv.test/embed/x",
            "https://www.youtube.com.evil.inv.test/embed/x",
            "https://evil.inv.test/?u=https://www.youtube.com/embed/x",
            "https://www.youtube.com@evil.inv.test/embed/x",
            "https://user@www.youtube.com/embed/x",
            "https://www.youtube.com:8443/embed/x",
            "http://www.youtube.com/embed/x",
            "javascript:alert(1)",
            " https://www.youtube.com/embed/x",
        ] {
            assert!(!valid_embed(bad, &providers), "{bad}");
        }
    }

    #[test]
    fn anchors_are_assigned_and_deduplicated() {
        let out = validate(
            &doc(vec![
                h("a", None),
                h("b", Some("keep-me")),
                h("c", Some("keep-me")),
                h("d", Some("bad id!")),
                h("e", None),
            ]),
            &body(),
        )
        .unwrap();
        let ids = anchors(&out);
        assert_eq!(ids.len(), 5);
        assert_eq!(ids[1], "keep-me");
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), 5, "{ids:?}");
        assert!(ids[0].starts_with("h-") && ids[3].starts_with("h-"));
        // 2回目の保存では変わらない
        let again = validate(&out, &body()).unwrap();
        assert_eq!(anchors(&again), ids);
    }

    #[test]
    fn problems_do_not_contain_content() {
        let secret = "SECRET-BODY-TEXT";
        let bad = doc(vec![
            json!({"type": "paragraph", "content": [{"type": "text", "text": secret, "marks": [{"type": "link", "attrs": {"href": format!("javascript:{secret}")}}]}]}),
        ]);
        let problems = validate(&bad, &body()).unwrap_err();
        assert!(!problems.is_empty());
        assert!(problems.iter().all(|p| !p.contains(secret)), "{problems:?}");
    }

    #[test]
    fn finds_media() {
        let d = doc(vec![
            json!({"type": "image", "attrs": {"mediaId": "0123456789abcdef0123456789abcdef"}}),
        ]);
        assert_eq!(media_ids(&d).len(), 1);
    }
}
