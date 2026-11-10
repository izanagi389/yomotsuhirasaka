import { describe, expect, it } from "vitest";
import { defineComponent, h } from "vue";
import { renderToString } from "vue/server-renderer";

import YomiBody, { type BodyNode, linkRel } from "../src/runtime/body.ts";

const render = (doc: BodyNode | null, components = {}) => renderToString(h(YomiBody, { doc, components }));
const text = (t: string, marks?: BodyNode["marks"]): BodyNode => ({ type: "text", text: t, marks });
const doc = (...content: BodyNode[]): BodyNode => ({ type: "doc", content });

describe("YomiBody", () => {
  it("意味のあるHTMLとして描き、見出しにアンカーIDを付ける", async () => {
    const html = await render(
      doc(
        { type: "heading", attrs: { level: 2, id: "h-abc" }, content: [text("見出し")] },
        { type: "paragraph", content: [text("太字", [{ type: "bold" }]), text(" と "), text("リンク", [{ type: "link", attrs: { href: "https://example.com/" } }])] },
        { type: "bulletList", content: [{ type: "listItem", content: [{ type: "paragraph", content: [text("項目")] }] }] },
        { type: "codeBlock", attrs: { language: "ts" }, content: [text("const a = 1;")] },
      ),
    );
    expect(html).toContain('<h2 id="h-abc">見出し</h2>');
    expect(html).toContain("<strong>太字</strong>");
    expect(html).toContain('<a href="https://example.com/">リンク</a>');
    expect(html).toContain("<ul><li><p>項目</p></li></ul>");
    expect(html).toContain('<pre><code data-language="ts">const a = 1;</code></pre>');
    expect(html).not.toMatch(/class=|style=/);
  });

  it("文字は文字として描く（HTMLとして解釈しない）", async () => {
    const html = await render(doc({ type: "paragraph", content: [text("<script>alert(1)</script>")] }));
    expect(html).not.toContain("<script>");
    expect(html).toContain("&lt;script&gt;");
  });

  it("使えないURLのリンクと画像は描かない", async () => {
    const html = await render(
      doc(
        { type: "paragraph", content: [text("x", [{ type: "link", attrs: { href: "javascript:alert(1)" } }])] },
        { type: "image", attrs: { src: "javascript:alert(1)", alt: "a" } },
        { type: "embed", attrs: { src: "javascript:alert(1)" } },
      ),
    );
    expect(html).not.toContain("javascript:");
    expect(html).toContain("<p>x</p>");
  });

  it("画像は代替テキストと大きさを付け、装飾画像は alt を空にする", async () => {
    const html = await render(
      doc(
        { type: "image", attrs: { src: "https://yomi.example.com/media/w/a.webp", alt: "説明", width: 40, height: 30 } },
        { type: "image", attrs: { src: "https://yomi.example.com/media/w/b.webp", alt: "使わない", decorative: true } },
      ),
    );
    expect(html).toContain('src="https://yomi.example.com/media/w/a.webp" alt="説明" width="40" height="30" loading="lazy"');
    // 空の alt（Vue は `alt` とだけ書く。HTMLでは alt="" と同じ）
    expect(html).toMatch(/src="https:\/\/yomi\.example\.com\/media\/w\/b\.webp" alt(="")? /);
    expect(html).not.toContain("使わない");
  });

  it("ノードとマークをサイトの部品で置き換えられる", async () => {
    const Embed = defineComponent({ props: { node: { type: Object, required: true } }, setup: (p) => () => h("iframe", { src: (p.node as BodyNode).attrs?.["src"] as string }) });
    const Strong = defineComponent({ setup: (_, { slots }) => () => h("b", { "data-x": "1" }, slots["default"]?.()) });
    const html = await render(
      doc({ type: "embed", attrs: { src: "https://www.youtube.com/embed/x" } }, { type: "paragraph", content: [text("強調", [{ type: "bold" }])] }),
      { embed: Embed, "mark:bold": Strong },
    );
    expect(html).toContain('<iframe src="https://www.youtube.com/embed/x"></iframe>');
    expect(html).toContain('<b data-x="1">強調</b>');
  });

  it("知らないノードは子だけを描き、本文がなければ何も描かない", async () => {
    expect(await render(doc({ type: "callout", content: [{ type: "paragraph", content: [text("中身")] }] }))).toContain("<p>中身</p>");
    expect(await render(null)).toBe("<!---->");
  });
});

describe("linkRel", () => {
  it("新しい文脈を開きうる target には noopener noreferrer を必ず付ける", () => {
    expect(linkRel("_blank", undefined)).toBe("noopener noreferrer");
    expect(linkRel("other-window", undefined)).toBe("noopener noreferrer");
    expect(linkRel("_BLANK", "nofollow")).toBe("nofollow noopener noreferrer");
    expect(linkRel("named", "opener")).toBe("opener noopener noreferrer");
    expect(linkRel("x", "noopener noreferrer")).toBe("noopener noreferrer");
  });
  it("同じ文脈にとどまる target では、指定された rel のまま", () => {
    for (const t of [undefined, "", "_self", "_parent", "_top"]) {
      expect(linkRel(t, undefined)).toBeUndefined();
      expect(linkRel(t, "nofollow")).toBe("nofollow");
    }
  });
});
