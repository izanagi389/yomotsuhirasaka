import { type JSONContent, getSchema } from "@tiptap/core";
import { describe, expect, it } from "vitest";

import { anchorFixes, bodyDefinition, forManifest, sanitize, standardExtensions, unknownTypes } from "../src/index.ts";
import type { ManifestBody } from "../src/index.ts";

/** テストの blog テンプレートと同じ形のマニフェストの本文（標準の一部だけ）。 */
const blogBody: ManifestBody = {
  nodes: [
    { name: "doc", children: ["paragraph", "heading", "bulletList", "image", "embed"] },
    { name: "paragraph", children: ["text"], marks: true },
    { name: "heading", children: ["text"], attrs: ["level"], marks: true },
    { name: "bulletList", children: ["listItem"] },
    { name: "listItem", children: ["paragraph", "bulletList"] },
    { name: "text" },
    { name: "image", attrs: ["mediaId", "alt", "decorative"] },
    { name: "embed", attrs: ["src"] },
  ],
  marks: [{ name: "bold" }, { name: "link", attrs: ["href", "target", "rel"] }],
  embedProviders: ["www.youtube.com"],
};

describe("bodyDefinition", () => {
  const def = bodyDefinition(standardExtensions());
  const node = (name: string) => def.nodes.find((n) => n.name === name);

  it("標準のノードとマークを書き出す", () => {
    expect(def.nodes.map((n) => n.name).sort()).toEqual(
      ["blockquote", "bulletList", "codeBlock", "doc", "embed", "hardBreak", "heading", "horizontalRule", "image", "listItem", "orderedList", "paragraph", "text"].sort(),
    );
    expect(def.marks.map((m) => m.name).sort()).toEqual(["bold", "code", "italic", "link", "strike"]);
  });

  it("CMSが意味を知っているノードは、サーバーが要る属性を持つ", () => {
    expect(node("image")?.attrs).toEqual(["mediaId", "alt", "decorative"]);
    expect(node("embed")?.attrs).toEqual(["src"]);
    // アンカーIDはサーバーが扱うので、定義には書かない
    expect(node("heading")?.attrs).toEqual(["level"]);
  });

  it("content式を子の一覧に簡略化する（Q142）", () => {
    const blocks = ["paragraph", "blockquote", "bulletList", "codeBlock", "heading", "horizontalRule", "listItem", "orderedList", "image", "embed"];
    expect(new Set(node("doc")?.children)).toEqual(new Set(blocks.filter((b) => b !== "listItem")));
    expect(node("bulletList")?.children).toEqual(["listItem"]);
    expect(node("listItem")?.children).toContain("paragraph");
    expect(node("paragraph")?.children.sort()).toEqual(["hardBreak", "text"]);
    expect(node("image")?.children).toEqual([]);
  });

  it("マークを付けられるかを書き出す", () => {
    expect(node("paragraph")?.marks).toBe(true);
    expect(node("heading")?.marks).toBe(true);
    expect(node("codeBlock")?.marks).toBe(false);
    expect(node("doc")?.marks).toBe(false);
  });
});

describe("forManifest", () => {
  it("マニフェストにあるノードとマークだけに絞る", () => {
    const narrowed = forManifest(standardExtensions(), blogBody);
    expect(narrowed.editable).toBe(true);
    expect(narrowed.unsupported).toEqual([]);
    const def = bodyDefinition(narrowed.extensions);
    expect(def.nodes.map((n) => n.name).sort()).toEqual(["bulletList", "doc", "embed", "heading", "image", "listItem", "paragraph", "text"]);
    expect(def.marks.map((m) => m.name).sort()).toEqual(["bold", "link"]);
  });

  it("知らないノードを知らせ、必須のノードがなければ編集させない", () => {
    const narrowed = forManifest(standardExtensions(), {
      nodes: [{ name: "doc", children: ["callout"] }, { name: "callout" }, { name: "text" }],
      marks: [{ name: "highlight" }],
    });
    expect(narrowed.unsupported).toEqual(["callout", "highlight"]);
    expect(narrowed.editable).toBe(false);
  });
});

describe("unknownTypes", () => {
  it("拡張機能が知らないノードとマークを返す", () => {
    const { extensions } = forManifest(standardExtensions(), blogBody);
    const doc = {
      type: "doc",
      content: [
        { type: "paragraph", content: [{ type: "text", text: "x", marks: [{ type: "italic" }] }] },
        { type: "callout" },
      ],
    };
    expect(unknownTypes(doc, extensions).sort()).toEqual(["callout", "italic"]);
  });
});

describe("アンカーIDと保存するJSON", () => {
  const schema = getSchema(forManifest(standardExtensions(), blogBody).extensions);
  // エディタと同じく、スキーマを通して既定の属性を補った文書にする
  const parse = (json: JSONContent) => schema.nodeFromJSON(json);

  it("IDのない見出しと、重複した2つ目以降の見出しを振り直す", () => {
    const doc = parse({
      type: "doc",
      content: [
        { type: "heading", attrs: { level: 2, id: "keep" }, content: [{ type: "text", text: "A" }] },
        { type: "heading", attrs: { level: 2, id: "keep" }, content: [{ type: "text", text: "B" }] },
        { type: "heading", attrs: { level: 3 }, content: [{ type: "text", text: "C" }] },
        { type: "heading", attrs: { level: 3, id: "bad id!" }, content: [{ type: "text", text: "D" }] },
        { type: "paragraph", content: [{ type: "text", text: "p" }] },
      ],
    });
    const fixes = anchorFixes(doc);
    expect(fixes.map((f) => f.pos)).toEqual([3, 6, 9]);
    const ids = new Set(["keep", ...fixes.map((f) => f.id)]);
    expect(ids.size).toBe(4);
    for (const f of fixes) expect(f.id).toMatch(/^h-[0-9a-z]{10}$/);
    expect(anchorFixes(parse({ type: "doc", content: [{ type: "heading", attrs: { level: 2, id: "x" } }] }))).toEqual([]);
  });

  it("マニフェストにない属性を取り除く", () => {
    const json = parse({
      type: "doc",
      content: [
        { type: "heading", attrs: { level: 2, id: "h-1" }, content: [{ type: "text", text: "A" }] },
        { type: "paragraph", content: [{ type: "text", text: "link", marks: [{ type: "link", attrs: { href: "https://example.com" } }] }] },
        { type: "image", attrs: { mediaId: "0e2f0f7e-8f1b-4c3a-9f0e-111111111111" } },
      ],
    }).toJSON() as JSONContent;
    // スキーマを通すと、Tiptap のリンクは class・title も持つ
    expect(Object.keys(json.content?.[1]?.content?.[0]?.marks?.[0]?.attrs ?? {}).sort()).toEqual(["class", "href", "rel", "target", "title"]);
    const clean = sanitize(json, blogBody);
    expect(clean.content?.[0]?.attrs).toEqual({ level: 2, id: "h-1" });
    // target・rel は既定で null（開き方はサイトが決める）
    expect(clean.content?.[1]?.content?.[0]?.marks).toEqual([{ type: "link", attrs: { href: "https://example.com", target: null, rel: null } }]);
    expect(clean.content?.[2]?.attrs).toEqual({ mediaId: "0e2f0f7e-8f1b-4c3a-9f0e-111111111111", alt: null, decorative: false });
    expect(clean.content?.[1]).not.toHaveProperty("attrs");
  });
});
