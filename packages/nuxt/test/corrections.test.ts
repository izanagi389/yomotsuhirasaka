import { describe, expect, it } from "vitest";
import { h } from "vue";
import { renderToString } from "vue/server-renderer";

import CmsCorrections from "../src/runtime/corrections.ts";
import CmsDeletedAnchorNotice, { type CmsDeletedAnchor, findDeletedAnchor } from "../src/runtime/deleted-anchor.ts";
import { type CmsCorrectionItem, FeedConfigError, checkFeedConfig, escapeXml, toJsonFeed, toRss } from "../src/runtime/server/feed.ts";

const feed = checkFeedConfig("ja", { path: "/corrections", title: "訂正の一覧", link: "https://example.com" });
const item = (over: Partial<CmsCorrectionItem> = {}): CmsCorrectionItem => ({
  entryId: "e-1",
  contentType: "post",
  url: "https://example.com/posts/a",
  title: "記事A",
  correctedAt: "2026-10-20T03:04:05Z",
  description: "数値を訂正しました",
  sections: ["h-1"],
  ...over,
});

describe("checkFeedConfig", () => {
  it("既定の件数と説明を補う", () => {
    expect(feed).toEqual({ locale: "ja", path: "/corrections", title: "訂正の一覧", link: "https://example.com/", description: "訂正の一覧", limit: 50 });
  });

  it("パス・URL・件数・ロケールの誤りはビルドで止める", () => {
    const ok = { path: "/c", title: "t", link: "https://example.com" };
    for (const [locale, bad] of [
      ["ja", { ...ok, path: "c" }],
      ["ja", { ...ok, path: "/c.xml" }],
      ["ja", { ...ok, path: "/a/../b" }],
      ["ja", { ...ok, path: "/a?x" }],
      ["ja", { ...ok, title: " " }],
      ["ja", { ...ok, link: "javascript:alert(1)" }],
      ["ja", { ...ok, limit: 101 }],
      ["ja", { ...ok, limit: 1.5 }],
      ["../x", ok],
    ] as const) {
      expect(() => checkFeedConfig(locale, bad)).toThrow(FeedConfigError);
    }
  });
});

describe("escapeXml", () => {
  it("記号をエスケープし、XMLで使えない文字を除く", () => {
    expect(escapeXml(`<a href="x">&'</a>`)).toBe("&lt;a href=&quot;x&quot;&gt;&amp;&apos;&lt;/a&gt;");
    expect(escapeXml("a\u0000b\u001Fc\td\ne￾f\uD800g")).toBe("abc\td\nefg");
    expect(escapeXml("😀")).toBe("😀");
  });
});

describe("toRss", () => {
  it("訂正ごとに1項目を書き、文字はエスケープする", () => {
    const xml = toRss(feed, [item({ title: "A & <B>", description: "]]></description><script>" }), item({ correctedAt: "2026-10-01T00:00:00Z", description: null })]);
    expect(xml).toContain("<title>A &amp; &lt;B&gt;</title>");
    expect(xml).toContain("<description>]]&gt;&lt;/description&gt;&lt;script&gt;</description>");
    expect(xml).not.toContain("<script>");
    expect(xml).toContain('<guid isPermaLink="false">e-1@2026-10-20T03:04:05Z</guid>');
    expect(xml).toContain("<pubDate>Tue, 20 Oct 2026 03:04:05 GMT</pubDate>");
    expect(xml).toContain("<lastBuildDate>Tue, 20 Oct 2026 03:04:05 GMT</lastBuildDate>");
    expect(xml).toContain('<atom:link href="https://example.com/corrections.xml" rel="self" type="application/rss+xml"/>');
    expect(xml.match(/<item>/g)).toHaveLength(2);
    // 説明文がない項目には description を書かない
    expect(xml.match(/<description>/g)).toHaveLength(2);
  });

  it("http(s) でないリンクや読めない日時の項目は書かない", () => {
    const xml = toRss(feed, [item({ url: "javascript:alert(1)" }), item({ correctedAt: "x" }), item({ url: "https://example.com/ok" })]);
    expect(xml.match(/<item>/g)).toHaveLength(1);
    expect(xml).not.toContain("javascript:");
  });
});

describe("toJsonFeed", () => {
  it("JSON Feed 1.1 として書き、セクションは拡張に入れる", () => {
    const json = JSON.parse(toJsonFeed(feed, [item(), item({ url: "data:text/html,x" })]));
    expect(json).toMatchObject({ version: "https://jsonfeed.org/version/1.1", title: "訂正の一覧", home_page_url: "https://example.com/", feed_url: "https://example.com/corrections.json", language: "ja" });
    expect(json.items).toEqual([
      {
        id: "e-1@2026-10-20T03:04:05Z",
        url: "https://example.com/posts/a",
        title: "記事A",
        content_text: "数値を訂正しました",
        date_published: "2026-10-20T03:04:05.000Z",
        _corrections: { entry_id: "e-1", content_type: "post", sections: ["h-1"] },
      },
    ]);
  });
});

describe("CmsCorrections", () => {
  it("飾りのないリストとして描き、文字は文字として描く", async () => {
    const html = await renderToString(h(CmsCorrections, { items: [item({ title: "<b>x</b>" }), item({ url: "javascript:alert(1)", description: null })] }));
    expect(html).toContain('<ol><li><time datetime="2026-10-20T03:04:05Z">2026-10-20</time> <a href="https://example.com/posts/a">&lt;b&gt;x&lt;/b&gt;</a><p>数値を訂正しました</p></li>');
    expect(html).not.toContain("javascript:");
    expect(html).not.toMatch(/class=|style=/);
  });

  it("1件ごとの描き方をスロットで置き換えられる。空なら何も描かない", async () => {
    const html = await renderToString(h(CmsCorrections, { items: [item()] }, { item: ({ item }: { item: CmsCorrectionItem }) => h("span", item.sections.join(",")) }));
    expect(html).toBe("<ol><li><span>h-1</span></li></ol>");
    expect(await renderToString(h(CmsCorrections, { items: [] }))).toBe("<!---->");
  });
});

describe("CmsDeletedAnchorNotice", () => {
  const anchors: CmsDeletedAnchor[] = [
    { anchor: "h-old", deletedAt: "2026-10-20T00:00:00Z", description: "訂正により削除されました" },
    { anchor: "見出し", deletedAt: "2026-10-20T00:00:00Z", description: null },
  ];

  it("URLの # が削除済みアンカーなら、その1件を返す", () => {
    expect(findDeletedAnchor("#h-old", anchors)?.description).toBe("訂正により削除されました");
    expect(findDeletedAnchor("#%E8%A6%8B%E5%87%BA%E3%81%97", anchors)?.anchor).toBe("見出し");
    expect(findDeletedAnchor("#h-new", anchors)).toBeUndefined();
    expect(findDeletedAnchor("", anchors)).toBeUndefined();
    expect(findDeletedAnchor("#", anchors)).toBeUndefined();
    expect(findDeletedAnchor("#%E0%A4%A", anchors)).toBeUndefined();
  });

  it("今の本文に同じIDがあれば知らせない", () => {
    expect(findDeletedAnchor("#h-old", anchors, (id) => id === "h-old")).toBeUndefined();
  });

  it("サーバーでの描画では何も出さない", async () => {
    expect(await renderToString(h(CmsDeletedAnchorNotice, { anchors }))).toBe("<!---->");
  });
});
