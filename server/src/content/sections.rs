//! 公開のときに本文から求めるもの: 空かどうか、代替テキストのない画像、アンカーID、セクションの比較。
//!
//! - セクションは、本文の最上位の見出しで区切る。見出しのアンカーIDがセクションの名前になる。最初の見出しより前の部分は「導入」とする。
//! - 「訂正として公開」で影響したセクションは、前の版と比べて内容が変わった・消えた・増えたセクションとする（06-corrections-and-freshness.md、Q42）。
//! - 画像は、空白だけでない代替テキストか「装飾画像」の印（`decorative: true`）のどちらかが要る（Q75、INV-IMG-01）。

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::body::{ANCHOR_ATTR, HEADING, IMAGE};
use crate::schema::manifest::Body;

fn children(v: &Value) -> &[Value] {
    v.get("content")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn anchor_of(node: &Value) -> Option<&str> {
    if node.get("type").and_then(Value::as_str) != Some(HEADING) {
        return None;
    }
    node.get("attrs")
        .and_then(|a| a.get(ANCHOR_ATTR))
        .and_then(Value::as_str)
}

/// 本文が空か（空白でない文字も、子を持たないノード（画像・埋め込みなど）もない）。
pub fn is_blank(doc: &Value, body: &Body) -> bool {
    let leaves: BTreeSet<&str> = body
        .nodes
        .iter()
        .filter(|n| n.children.is_empty() && n.name != "text")
        .map(|n| n.name.as_str())
        .collect();
    fn has_content(v: &Value, leaves: &BTreeSet<&str>) -> bool {
        match v.get("type").and_then(Value::as_str) {
            Some("text") => v
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|t| !t.trim().is_empty()),
            Some(ty) if leaves.contains(ty) => true,
            Some(_) => children(v).iter().any(|c| has_content(c, leaves)),
            None => false,
        }
    }
    !has_content(doc, &leaves)
}

/// 代替テキストも「装飾画像」の印もない画像の場所（JSONのパス）。
pub fn images_missing_alt(doc: &Value) -> Vec<String> {
    fn walk(v: &Value, path: &str, out: &mut Vec<String>) {
        if v.get("type").and_then(Value::as_str) == Some(IMAGE) {
            let attrs = v.get("attrs");
            let alt = attrs
                .and_then(|a| a.get("alt"))
                .and_then(Value::as_str)
                .is_some_and(|s| !s.trim().is_empty());
            let decorative = attrs.and_then(|a| a.get("decorative")) == Some(&Value::Bool(true));
            if !alt && !decorative {
                out.push(path.to_owned());
            }
        }
        for (i, c) in children(v).iter().enumerate() {
            walk(c, &format!("{path}.content[{i}]"), out);
        }
    }
    let mut out = Vec::new();
    walk(doc, "$", &mut out);
    out
}

/// 本文の見出しのアンカーID（出てきた順。入れ子の見出しも含む）。
pub fn anchors(doc: &Value) -> Vec<String> {
    fn walk(v: &Value, out: &mut Vec<String>) {
        if let Some(id) = anchor_of(v) {
            out.push(id.to_owned());
        }
        for c in children(v) {
            walk(c, out);
        }
    }
    let mut out = Vec::new();
    walk(doc, &mut out);
    out
}

/// 最上位の見出しで区切ったセクション（導入は None）→ そのセクションのノードの列。
fn sections(doc: &Value) -> BTreeMap<Option<String>, Vec<&Value>> {
    let mut out: BTreeMap<Option<String>, Vec<&Value>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for node in children(doc) {
        if let Some(id) = anchor_of(node) {
            current = Some(id.to_owned());
        }
        out.entry(current.clone()).or_default().push(node);
    }
    out
}

/// 前の本文と比べて影響したセクション。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Affected {
    /// 変わった・消えた・増えたセクションのアンカーID。
    pub anchors: BTreeSet<String>,
    /// 導入（最初の見出しより前の部分）が変わったか。
    pub lead: bool,
}

/// 本文の一覧（フィールドの順）どうしを比べる。
pub fn affected(before: &[&Value], after: &[&Value]) -> Affected {
    let mut out = Affected::default();
    let mut lead_before = Vec::new();
    let mut lead_after = Vec::new();
    let mut old: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    let mut new: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for (docs, lead, named) in [
        (before, &mut lead_before, &mut old),
        (after, &mut lead_after, &mut new),
    ] {
        for (i, doc) in docs.iter().enumerate() {
            for (key, nodes) in sections(doc) {
                match key {
                    None => lead.push((i, nodes)),
                    Some(id) => {
                        named.insert(id, nodes);
                    }
                }
            }
        }
    }
    out.lead = lead_before != lead_after;
    let keys: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    for k in keys {
        if old.get(k) != new.get(k) {
            out.anchors.insert(k.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn p(t: &str) -> Value {
        json!({"type": "paragraph", "content": [{"type": "text", "text": t}]})
    }
    fn h(t: &str, id: &str) -> Value {
        json!({"type": "heading", "attrs": {"level": 2, "id": id}, "content": [{"type": "text", "text": t}]})
    }
    fn doc(nodes: Vec<Value>) -> Value {
        json!({"type": "doc", "content": nodes})
    }

    #[test]
    fn blank_bodies() {
        let body: Body = serde_json::from_value(json!({
            "nodes": [
                {"name": "doc", "children": ["paragraph", "heading", "image"]},
                {"name": "paragraph", "children": ["text"]},
                {"name": "heading", "children": ["text"], "attrs": ["level"]},
                {"name": "text"},
                {"name": "image", "attrs": ["mediaId", "alt", "decorative"]}
            ],
            "marks": []
        }))
        .unwrap();
        let empty_heading = json!({"type": "heading", "attrs": {"level": 2, "id": "a"}});
        assert!(is_blank(&doc(vec![]), &body));
        assert!(is_blank(
            &doc(vec![json!({"type": "paragraph"}), p("  "), empty_heading]),
            &body
        ));
        assert!(!is_blank(&doc(vec![p("x")]), &body));
        assert!(!is_blank(
            &doc(vec![json!({"type": "image", "attrs": {"mediaId": "m"}})]),
            &body
        ));
    }

    #[test]
    fn images_need_alt_or_decorative() {
        let img = |attrs: Value| json!({"type": "image", "attrs": attrs});
        let d = doc(vec![
            img(json!({"mediaId": "m", "alt": "説明"})),
            img(json!({"mediaId": "m", "decorative": true})),
            img(json!({"mediaId": "m"})),
            img(json!({"mediaId": "m", "alt": "  ", "decorative": false})),
            json!({"type": "bulletList", "content": [{"type": "listItem", "content": [img(json!({"mediaId": "m", "decorative": "true"}))]}]}),
        ]);
        assert_eq!(
            images_missing_alt(&d),
            [
                "$.content[2]",
                "$.content[3]",
                "$.content[4].content[0].content[0]"
            ]
        );
    }

    #[test]
    fn affected_sections() {
        let before = doc(vec![
            p("導入"),
            h("A", "a"),
            p("1"),
            h("B", "b"),
            p("2"),
            h("C", "c"),
        ]);
        // a は変わらない、b は本文が変わる、c は消える、d は増える。導入は変わらない
        let after = doc(vec![
            p("導入"),
            h("A", "a"),
            p("1"),
            h("B", "b"),
            p("2 訂正"),
            h("D", "d"),
        ]);
        let a = affected(&[&before], &[&after]);
        assert_eq!(a.anchors.into_iter().collect::<Vec<_>>(), ["b", "c", "d"]);
        assert!(!a.lead);
        // 見出しの文字だけが変わってもそのセクションは影響したとみなす。導入の変更は lead
        let after2 = doc(vec![
            p("導入を直した"),
            h("A2", "a"),
            p("1"),
            h("B", "b"),
            p("2"),
            h("C", "c"),
        ]);
        let a = affected(&[&before], &[&after2]);
        assert_eq!(a.anchors.into_iter().collect::<Vec<_>>(), ["a"]);
        assert!(a.lead);
        assert_eq!(affected(&[&before], &[&before]), Affected::default());
        assert_eq!(anchors(&before), ["a", "b", "c"]);
    }
}
