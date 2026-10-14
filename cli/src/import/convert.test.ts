// 本文の変換と独自部品の検出の単体テスト（node:test）。
import { test } from "node:test";
import assert from "node:assert/strict";
import { parseAttrs, prepare } from "./components.js";
import { convert } from "./markdown.js";

const rules = {
  nodes: new Set(["doc", "paragraph", "text", "heading", "bulletList", "listItem", "image", "blockquote", "hardBreak", "codeBlock"]),
  marks: new Set(["bold", "link"]),
};

function run(body: string, map: Record<string, string> = {}) {
  const p = prepare("a.md", body, 5, map);
  const c = convert("a.md", p.text, p.lineOf, p.components, map, rules);
  return { ...c, problems: [...p.problems, ...c.problems], warnings: [...p.warnings, ...c.warnings] };
}

test("paragraphs, headings, lists, marks and links", () => {
  const r = run("# 見出し\n\n**太字** と [リンク](https://example.com/)\n\n- 一つ\n- 二つ\n");
  assert.deepEqual(r.problems, []);
  assert.deepEqual(r.doc.content, [
    { type: "heading", attrs: { level: 1 }, content: [{ type: "text", text: "見出し" }] },
    {
      type: "paragraph",
      content: [
        { type: "text", text: "太字", marks: [{ type: "bold" }] },
        { type: "text", text: " と " },
        { type: "text", text: "リンク", marks: [{ type: "link", attrs: { href: "https://example.com/" } }] },
      ],
    },
    {
      type: "bulletList",
      content: [
        { type: "listItem", content: [{ type: "paragraph", content: [{ type: "text", text: "一つ" }] }] },
        { type: "listItem", content: [{ type: "paragraph", content: [{ type: "text", text: "二つ" }] }] },
      ],
    },
  ]);
});

test("line breaks inside Japanese text are joined without a space", () => {
  const r = run("日本語の\n文章 and\nEnglish");
  assert.deepEqual(r.doc.content, [{ type: "paragraph", content: [{ type: "text", text: "日本語の文章 and English" }] }]);
});

test("images move out of paragraphs; missing alt text becomes decorative with a warning", () => {
  const r = run("前 ![猫](./cat.png) 後\n\n![](./dog.png)");
  assert.deepEqual(r.problems, []);
  assert.deepEqual(r.doc.content?.map((n) => n.type), ["paragraph", "image", "paragraph", "image"]);
  assert.deepEqual(r.images.map((i) => [i.src, i.node.attrs]), [
    ["./cat.png", { mediaId: "", alt: "猫" }],
    ["./dog.png", { mediaId: "", decorative: true }],
  ]);
  assert.equal(r.warnings.length, 1);
});

test("markup the template does not allow is reported with its line", () => {
  const r = run("ok\n\n*italic*\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n<div>x</div>\n");
  assert.ok(r.problems.some((p) => p.startsWith("a.md:7:") && p.includes('"italic"')), r.problems.join("\n"));
  assert.ok(r.problems.some((p) => p.includes("tables are not supported")));
  assert.ok(r.problems.some((p) => p.includes("component div has no mapping")));
});

test("unmapped components stop the import (JSX and MDC); code blocks are not scanned", () => {
  const r = run('前置き\n\n<Callout type="warning">注意</Callout>\n\n::alert{type="info"}\n本文\n::\n\n```\n<Fake/>\n::nope\n```\n');
  assert.deepEqual(r.problems, ["a.md:7: component Callout has no mapping in \"componentMap\"", "a.md:9: component alert has no mapping in \"componentMap\""]);
});

test("mapped components become nodes with their attributes and children", () => {
  const r = run('::alert{type="info" open}\n**中身**\n::\n\n<Note>\n\nnote\n\n</Note>', { alert: "blockquote", Note: "blockquote" });
  assert.deepEqual(r.problems, []);
  assert.deepEqual(r.doc.content, [
    { type: "blockquote", attrs: { type: "info", open: true }, content: [{ type: "paragraph", content: [{ type: "text", text: "中身", marks: [{ type: "bold" }] }] }] },
    { type: "blockquote", content: [{ type: "paragraph", content: [{ type: "text", text: "note" }] }] },
  ]);
});

test("inline components, unclosed components and expressions are refused", () => {
  assert.ok(run("text :badge[new] text").problems[0]?.includes("inline component :badge"));
  assert.ok(run("::alert\nbody").problems.some((p) => p.includes("component alert is not closed")));
  assert.ok(run("<Chart data={[1, 2]} />", { Chart: "blockquote" }).problems[0]?.includes("cannot be imported"));
  assert.deepEqual(parseAttrs('a="1" b c=\'x\''), { a: "1", b: true, c: "x" });
  assert.equal(parseAttrs("{expr}"), null);
});
