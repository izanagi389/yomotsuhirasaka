//! 変換器が出したHTMLの無害化（13-security.md「本文」、INV-CNV-01）。
//!
//! 変換器は本文中の文字列をすべてエスケープするが、カスタムノードの `toHtml` の出力はそのまま入る。そのため、
//! 配信する前に、出力の全体を ammonia（許可リスト方式）に通す。
//! - 許すのは文章の構造の要素だけ。`script`・`style`・`iframe`・`svg`・`math`・フォームなどは許さない
//!   （SVG・MathML を経由した mXSS の報告が続いているため、どちらも使わせない）。
//! - 属性は、要素ごとに必要なものと、`id`・`class`・`title`・`data-*` だけ。`style` とイベントの属性は許さない。
//! - URLは http(s)・mailto・tel と相対パスだけ。リンクには `rel="noopener noreferrer"` を付ける。

use std::collections::HashSet;
use std::sync::OnceLock;

use ammonia::{Builder, UrlRelative};

const TAGS: &[&str] = &[
    "p",
    "br",
    "hr",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "strong",
    "b",
    "em",
    "i",
    "s",
    "del",
    "ins",
    "u",
    "code",
    "pre",
    "kbd",
    "samp",
    "blockquote",
    "ul",
    "ol",
    "li",
    "dl",
    "dt",
    "dd",
    "a",
    "img",
    "span",
    "div",
    "figure",
    "figcaption",
    "table",
    "caption",
    "thead",
    "tbody",
    "tfoot",
    "tr",
    "th",
    "td",
    "sup",
    "sub",
    "mark",
    "small",
    "abbr",
    "aside",
    "section",
    "details",
    "summary",
];

fn builder() -> &'static Builder<'static> {
    static BUILDER: OnceLock<Builder<'static>> = OnceLock::new();
    BUILDER.get_or_init(|| {
        let mut b = Builder::empty();
        b.tags(TAGS.iter().copied().collect::<HashSet<_>>())
            .generic_attributes(["id", "class", "title"].into_iter().collect())
            .generic_attribute_prefixes(["data-"].into_iter().collect())
            .add_tag_attributes("a", ["href"])
            .add_tag_attributes("img", ["src", "alt", "width", "height"])
            .add_tag_attributes("ol", ["start"])
            .add_tag_attributes("th", ["colspan", "rowspan"])
            .add_tag_attributes("td", ["colspan", "rowspan"])
            .url_schemes(["http", "https", "mailto", "tel"].into_iter().collect())
            .url_relative(UrlRelative::PassThrough)
            .link_rel(Some("noopener noreferrer"))
            .strip_comments(true);
        b
    })
}

/// HTMLを無害化する。
pub fn sanitize(html: &str) -> String {
    builder().clean(html).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_content_is_removed() {
        for (input, banned) in [
            ("<script>alert(1)</script>x", "<script"),
            ("<img src=x onerror=alert(1)>", "onerror"),
            ("<a href=\"javascript:alert(1)\">a</a>", "javascript:"),
            ("<a href=\"java&#x09;script:alert(1)\">a</a>", "script:"),
            ("<a href=\"data:text/html,x\">a</a>", "data:"),
            ("<div style=\"x\" onclick=\"y\">d</div>", "style"),
            ("<svg><animate onbegin=\"alert(1)\"/></svg>", "animate"),
            (
                "<math><annotation-xml encoding=\"text/html\"><script>1</script></annotation-xml></math>",
                "script",
            ),
            ("<iframe srcdoc=\"x\"></iframe>", "iframe"),
            (
                "<form action=\"javascript:1\"><button>e</button></form>",
                "form",
            ),
            ("<!--<script>1</script>-->", "<!--"),
            (
                "<noscript><p title=\"</noscript><img src=x onerror=alert(1)>\"></noscript>",
                "onerror",
            ),
            ("<style>*{}</style>", "<style"),
        ] {
            let out = sanitize(input);
            assert!(
                !out.to_ascii_lowercase().contains(banned),
                "{input} -> {out}"
            );
        }
    }

    #[test]
    fn structure_is_kept() {
        let html = "<h2 id=\"a\">T</h2><p><a href=\"https://e.test/x\">l</a> <code class=\"language-js\">c</code></p><img src=\"/media/x.png\" alt=\"a\">";
        let out = sanitize(html);
        for keep in [
            "<h2 id=\"a\">",
            "href=\"https://e.test/x\"",
            "rel=\"noopener noreferrer\"",
            "class=\"language-js\"",
            "src=\"/media/x.png\"",
        ] {
            assert!(out.contains(keep), "{keep}: {out}");
        }
    }
}
