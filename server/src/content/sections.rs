//! 公開のときに本文から求めるもの: 空かどうか、代替テキストのない画像、アンカーID、セクションの比較。
//!
//! - セクションは、本文の最上位の見出しで区切る。見出しのアンカーIDがセクションの名前になる。最初の見出しより前の部分は「導入」とする。
//! - 「訂正として公開」で影響したセクションは、前の版と比べて内容が変わった・消えた・増えたセクションとする（06-corrections-and-freshness.md、Q42）。
//! - 画像は、空白だけでない代替テキストか「装飾画像」の印（`decorative: true`）のどちらかが要る（Q75、INV-IMG-01）。

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
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

/// 画像の属性（本文の画像のノードの `attrs`、または画像のフィールドの値）に、空白だけでない代替テキストか
/// 「装飾画像」の印があるか（Q75、INV-IMG-01）。
pub fn has_alt(attrs: &Value) -> bool {
    let alt = attrs
        .get("alt")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.trim().is_empty());
    alt || attrs.get("decorative") == Some(&Value::Bool(true))
}

/// 代替テキストも「装飾画像」の印もない画像の場所（JSONのパス）。
pub fn images_missing_alt(doc: &Value) -> Vec<String> {
    fn walk(v: &Value, path: &str, out: &mut Vec<String>) {
        if v.get("type").and_then(Value::as_str) == Some(IMAGE)
            && !v.get("attrs").is_some_and(has_alt)
        {
            out.push(path.to_owned());
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

/// セクションの変わり方。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    Changed,
    Added,
    Removed,
}

/// 変わったセクション（公開の確認画面に出す。Q68）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionChange {
    /// 見出しのアンカーID。導入（最初の見出しより前の部分）は None。
    pub anchor: Option<String>,
    /// 導入のときの、本文のフィールドの番号（`affected` に渡した一覧の中の位置）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lead_of: Option<usize>,
    /// 見出しの文字列（新しい本文から。消えたセクションは前の本文から）。導入は None。
    pub title: Option<String>,
    pub kind: ChangeKind,
    /// 前にもあったセクションの中で、数値（日付を含む）かリンク先が変わったか（「誤りの訂正ですか？」のヒント。Q68）。
    pub numbers_or_links_changed: bool,
}

/// ノードの列の文字列（見出しの文字列を取り出すため）。
fn plain_text(v: &Value, out: &mut String) {
    if let Some(t) = v.get("text").and_then(Value::as_str) {
        out.push_str(t);
    }
    for c in children(v) {
        plain_text(c, out);
    }
}

/// ノードの列の中の数値（数字の並び）とリンク先を、出てきた順に集める。
fn numbers_and_links(nodes: &[&Value]) -> (Vec<String>, Vec<String>) {
    fn walk(v: &Value, numbers: &mut Vec<String>, links: &mut Vec<String>) {
        if let Some(t) = v.get("text").and_then(Value::as_str) {
            let mut run = String::new();
            for c in t.chars() {
                // 全角の数字も半角の数字として扱う
                let digit = match c {
                    '0'..='9' => Some(c),
                    '０'..='９' => {
                        char::from_u32(u32::from(c) - u32::from('０') + u32::from('0'))
                    }
                    _ => None,
                };
                if let Some(d) = digit {
                    run.push(d);
                } else if !run.is_empty() {
                    numbers.push(std::mem::take(&mut run));
                }
            }
            if !run.is_empty() {
                numbers.push(run);
            }
        }
        for m in v
            .get("marks")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice)
        {
            if let Some(href) = m
                .get("attrs")
                .and_then(|a| a.get("href"))
                .and_then(Value::as_str)
            {
                links.push(href.to_owned());
            }
        }
        for c in children(v) {
            walk(c, numbers, links);
        }
    }
    let (mut numbers, mut links) = (Vec::new(), Vec::new());
    for n in nodes {
        walk(n, &mut numbers, &mut links);
    }
    // 同じリンクが続く（1つのリンクが複数のテキストのノードに分かれた）ときは1つにまとめる
    links.dedup();
    (numbers, links)
}

fn change(before: Option<&Vec<&Value>>, after: Option<&Vec<&Value>>) -> Option<(ChangeKind, bool)> {
    match (before, after) {
        (Some(b), Some(a)) if b == a => None,
        (Some(b), Some(a)) => Some((
            ChangeKind::Changed,
            numbers_and_links(b) != numbers_and_links(a),
        )),
        (Some(_), None) => Some((ChangeKind::Removed, false)),
        (None, Some(_)) => Some((ChangeKind::Added, false)),
        (None, None) => None,
    }
}

/// 本文の一覧（フィールドの順）どうしを比べ、変わったセクションを返す（導入、続いて見出しの順）。
pub fn compare(before: &[&Value], after: &[&Value]) -> Vec<SectionChange> {
    type Leads<'a> = BTreeMap<usize, Vec<&'a Value>>;
    type Named<'a> = BTreeMap<String, Vec<&'a Value>>;
    fn split<'a>(docs: &[&'a Value]) -> (Leads<'a>, Named<'a>) {
        let (mut leads, mut named) = (Leads::new(), Named::new());
        for (i, doc) in docs.iter().enumerate() {
            for (key, nodes) in sections(doc) {
                match key {
                    None => {
                        leads.insert(i, nodes);
                    }
                    Some(id) => {
                        named.insert(id, nodes);
                    }
                }
            }
        }
        (leads, named)
    }
    // 出てきた順（セクションの BTreeMap は名前の順なので、本文の順を別に持つ）
    fn ordered_anchors(docs: &[&Value]) -> Vec<String> {
        let mut out = Vec::new();
        for doc in docs {
            for node in children(doc) {
                if let Some(id) = anchor_of(node) {
                    out.push(id.to_owned());
                }
            }
        }
        out
    }
    let (old_leads, old) = split(before);
    let (new_leads, new) = split(after);
    let title = |nodes: Option<&Vec<&Value>>| {
        nodes.and_then(|n| n.first()).map(|h| {
            let mut s = String::new();
            plain_text(h, &mut s);
            s
        })
    };
    let mut out = Vec::new();
    let lead_keys: BTreeSet<usize> = old_leads.keys().chain(new_leads.keys()).copied().collect();
    for i in lead_keys {
        if let Some((kind, hint)) = change(old_leads.get(&i), new_leads.get(&i)) {
            out.push(SectionChange {
                anchor: None,
                lead_of: Some(i),
                title: None,
                kind,
                numbers_or_links_changed: hint,
            });
        }
    }
    let mut seen = BTreeSet::new();
    let mut keys = ordered_anchors(after);
    keys.extend(ordered_anchors(before));
    for k in keys {
        if !seen.insert(k.clone()) {
            continue;
        }
        if let Some((kind, hint)) = change(old.get(&k), new.get(&k)) {
            let t = if kind == ChangeKind::Removed {
                title(old.get(&k))
            } else {
                title(new.get(&k))
            };
            out.push(SectionChange {
                anchor: Some(k),
                lead_of: None,
                title: t,
                kind,
                numbers_or_links_changed: hint,
            });
        }
    }
    out
}

/// 本文の一覧（フィールドの順）の中の、あるセクションの文字列（最上位のノードごとに改行で区切る）。
/// `change` の示すセクション（導入はフィールドの番号、それ以外はアンカーID）がなければ None。
pub fn section_text(docs: &[&Value], change: &SectionChange) -> Option<String> {
    let key = change.anchor.clone();
    let nodes = match change.lead_of {
        Some(i) => docs.get(i).and_then(|d| sections(d).remove(&None)),
        None => docs.iter().find_map(|d| sections(d).remove(&key)),
    }?;
    let lines: Vec<String> = nodes
        .into_iter()
        .map(|n| {
            let mut s = String::new();
            plain_text(n, &mut s);
            s
        })
        .collect();
    Some(lines.join("\n"))
}

/// 本文の一覧（フィールドの順）どうしを比べる（「訂正として公開」で版に記録する）。
pub fn affected(before: &[&Value], after: &[&Value]) -> Affected {
    let mut out = Affected::default();
    for c in compare(before, after) {
        match c.anchor {
            Some(a) => {
                out.anchors.insert(a);
            }
            None => out.lead = true,
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

    #[test]
    fn text_of_a_section() {
        let before = doc(vec![p("導入"), h("A", "a"), p("前の文"), p("2つめ")]);
        let after = doc(vec![
            p("導入"),
            h("A", "a"),
            p("今の文"),
            h("B", "b"),
            p("足した"),
        ]);
        let changes = compare(&[&before], &[&after]);
        let a = changes
            .iter()
            .find(|c| c.anchor.as_deref() == Some("a"))
            .unwrap();
        assert_eq!(
            section_text(&[&before], a).as_deref(),
            Some("A\n前の文\n2つめ")
        );
        assert_eq!(section_text(&[&after], a).as_deref(), Some("A\n今の文"));
        let b = changes
            .iter()
            .find(|c| c.anchor.as_deref() == Some("b"))
            .unwrap();
        assert_eq!(section_text(&[&before], b), None);
        assert!(
            changes.iter().all(|c| c.anchor.is_some()),
            "導入は変わっていない"
        );
    }

    #[test]
    fn section_changes_for_the_publish_check() {
        let link = |t: &str, href: &str| json!({"type": "paragraph", "content": [{"type": "text", "text": t, "marks": [{"type": "link", "attrs": {"href": href}}]}]});
        let before = doc(vec![
            p("導入"),
            h("価格", "a"),
            p("月額 1,000 円です"),
            h("リンク", "b"),
            link("公式", "https://example.com/old"),
            h("文", "c"),
            p("とても良い"),
            h("消える", "d"),
        ]);
        let after = doc(vec![
            p("導入"),
            h("新しい", "e"),
            h("価格", "a"),
            p("月額 １，２００ 円です"),
            h("リンク", "b"),
            link("公式サイト", "https://example.com/new"),
            h("文", "c"),
            p("とても良かった"),
        ]);
        let changes = compare(&[&before], &[&after]);
        let summary: Vec<(Option<&str>, Option<&str>, ChangeKind, bool)> = changes
            .iter()
            .map(|c| {
                (
                    c.anchor.as_deref(),
                    c.title.as_deref(),
                    c.kind,
                    c.numbers_or_links_changed,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (Some("e"), Some("新しい"), ChangeKind::Added, false),
                (Some("a"), Some("価格"), ChangeKind::Changed, true),
                (Some("b"), Some("リンク"), ChangeKind::Changed, true),
                (Some("c"), Some("文"), ChangeKind::Changed, false),
                (Some("d"), Some("消える"), ChangeKind::Removed, false),
            ]
        );
        // 全角の数字は半角と同じに扱う（表記だけの変更ではヒントを出さない）
        let half = doc(vec![p("2026年10月15日")]);
        let full = doc(vec![p("２０２６年１０月１５日に")]);
        let c = compare(&[&half], &[&full]);
        assert_eq!(c.len(), 1);
        assert_eq!(
            (
                c[0].anchor.as_deref(),
                c[0].lead_of,
                c[0].numbers_or_links_changed
            ),
            (None, Some(0), false)
        );
        // affected は compare と同じ判定
        let a = affected(&[&before], &[&after]);
        assert_eq!(
            a.anchors.into_iter().collect::<Vec<_>>(),
            ["a", "b", "c", "d", "e"]
        );
        assert!(!a.lead);
    }
}
