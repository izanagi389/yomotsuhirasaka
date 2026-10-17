import { describe, expect, test } from "vitest";
import { convert, escapeHtml, MEDIA_SCHEME, plainText, renderFields, type Node } from "../src/index.ts";

const t = (text: string, marks?: Node["marks"]): Node => (marks ? { type: "text", text, marks } : { type: "text", text });
const p = (...content: Node[]): Node => ({ type: "paragraph", content });
const doc = (...content: Node[]): Node => ({ type: "doc", content });

describe("HTML", () => {
  test("escapes text and attribute values", () => {
    const out = convert(
      doc(
        { type: "heading", attrs: { level: 2, id: `a"><script>` }, content: [t("<script>alert(1)</script>")] },
        p(t(`"><img onerror=x>`, [{ type: "link", attrs: { href: `https://e.test/?q="><b>` } }])),
        { type: "codeBlock", attrs: { language: `x"><script>` }, content: [t("</code><script>")] },
        { type: "image", attrs: { mediaId: `m"><x`, alt: `"onerror=x` } },
      ),
      "html",
    );
    expect(out).not.toMatch(/<script|<img onerror|<b>|"onerror/);
    expect(out).toContain("&#60;script&#62;");
    expect(out).toContain(`<pre><code class="language-xscript">`);
    expect(out).toContain(`src="${MEDIA_SCHEME}m&#34;&#62;&#60;x"`);
  });

  test("renders the standard structure", () => {
    const out = convert(
      doc(
        { type: "bulletList", content: [{ type: "listItem", content: [p(t("a", [{ type: "bold" }]))] }] },
        { type: "orderedList", attrs: { start: 3 }, content: [{ type: "listItem", content: [p(t("b"))] }] },
        { type: "blockquote", content: [p(t("q"), { type: "hardBreak" }, t("r"))] },
        { type: "horizontalRule" },
        { type: "embed", attrs: { src: "https://www.youtube.com/embed/x" } },
      ),
      "html",
    );
    expect(out).toBe(
      [
        "<ul><li><p><strong>a</strong></p></li></ul>",
        `<ol start="3"><li><p>b</p></li></ol>`,
        "<blockquote><p>q<br>r</p></blockquote>",
        "<hr>",
        `<p><a href="https://www.youtube.com/embed/x">https://www.youtube.com/embed/x</a></p>`,
      ].join("\n"),
    );
  });

  test("custom nodes: toHtml as is, otherwise the escaped Markdown in a wrapper", () => {
    const custom = {
      callout: { toMarkdown: (n: Node) => `> ${String(n.attrs?.["label"])}`, toHtml: () => "<aside>raw</aside>" },
      note: { toMarkdown: (n: Node) => `<${String(n.attrs?.["label"])}>` },
      badge: { inline: true, toMarkdown: (_n: Node, ctx: { children(): string }) => `[${ctx.children()}]` },
    };
    const d = doc({ type: "callout", attrs: { label: "x" } }, { type: "note", attrs: { label: "y" } }, p({ type: "badge", content: [t("*b*")] }));
    expect(convert(d, "html", custom)).toBe(`<aside>raw</aside>\n<div data-node="note">&#60;y&#62;</div>\n<p><span data-node="badge">[\\*b\\*]</span></p>`);
    expect(convert(d, "markdown", custom)).toBe("> x\n\n<y>\n\n[\\*b\\*]");
  });

  test("nodes without a conversion function only keep their text", () => {
    expect(convert(doc({ type: "unknown", content: [t("<b>x</b>")] }), "html")).toBe("&#60;b&#62;x&#60;/b&#62;");
    expect(convert(doc({ type: "__proto__", content: [t("x")] }), "html")).toBe("x");
  });
});

describe("Markdown", () => {
  test("headings keep their anchor IDs, text is escaped", () => {
    const out = convert(
      doc(
        { type: "heading", attrs: { level: 3, id: "abc" }, content: [t("A *b*")] },
        p(t("1. not a list [x](y)"), t("code`s", [{ type: "code" }]), t("go", [{ type: "link", attrs: { href: "https://e.test/a b" } }])),
        { type: "codeBlock", attrs: { language: "js" }, content: [t("```\nx")] },
        { type: "image", attrs: { mediaId: "m1", alt: "a]b" } },
      ),
      "markdown",
    );
    expect(out).toBe(
      [
        "### A \\*b\\* {#abc}",
        "1\\. not a list \\[x\\]\\(y\\)``code`s``[go](<https://e.test/a b>)",
        "````js\n```\nx\n````",
        `![a\\]b](${MEDIA_SCHEME}m1)`,
      ].join("\n\n"),
    );
  });

  test("lists nest", () => {
    const li = (...c: Node[]): Node => ({ type: "listItem", content: c });
    const out = convert(doc({ type: "bulletList", content: [li(p(t("a")), { type: "orderedList", content: [li(p(t("b"))), li(p(t("c")))] })] }), "markdown");
    expect(out).toBe("- a\n\n  1. b\n  2. c");
  });
});

test("renderFields converts every field to both formats", () => {
  expect(renderFields({ body: doc(p(t("x"))) })).toEqual({ body: { markdown: "x", html: "<p>x</p>" } });
  expect(plainText(doc(p(t("a")), p(t("b"))))).toBe("a\n\nb");
  expect(escapeHtml(`&<>"'\``)).toBe("&#38;&#60;&#62;&#34;&#39;&#96;");
});
