import { describe, expect, it } from "vitest";

import { AiConfigError, type CmsAiCatalogItem, DEFAULT_AI_TRAINING_CRAWLERS, articlePath, checkAiConfig, markdownPath, toLlmsTxt, toRobotsTxt } from "../src/runtime/server/ai.ts";

const SITE = "https://example.com";
const config = checkAiConfig(
  { locales: { ja: { title: "例のブログ", description: "技術の記事" }, en: { title: "English articles" } }, robotsTxt: { addCrawlers: ["MyBot"], removeCrawlers: ["ccbot"], extra: "User-agent: *\nDisallow: /admin" } },
  `${SITE}/`,
);
const item = (url: string, title: string | null = "記事"): CmsAiCatalogItem => ({ entryId: "e", contentType: "post", url, title, locale: "ja", updatedAt: "2026-10-20T00:00:00Z" });

describe("checkAiConfig", () => {
  it("既定値を補い、クローラーの一覧に足し引きする", () => {
    expect(config.siteOrigin).toBe(SITE);
    expect(config.locales.map((l) => l.locale)).toEqual(["ja", "en"]);
    expect(config.llmsTxt).toBe(true);
    expect(config.markdown).toBe(true);
    expect(config.robotsTxt?.crawlers).toContain("MyBot");
    expect(config.robotsTxt?.crawlers).not.toContain("CCBot");
    expect(config.robotsTxt?.crawlers).toContain("GPTBot");
    expect(checkAiConfig({ locales: { ja: { title: "x" } } }, SITE).robotsTxt).toBeNull();
  });

  it("サイトのURL・ロケール・クローラーの誤りはビルドで止める", () => {
    expect(() => checkAiConfig({ locales: { ja: { title: "x" } } }, "")).toThrow(AiConfigError);
    expect(() => checkAiConfig({ locales: {} }, SITE)).toThrow(AiConfigError);
    expect(() => checkAiConfig({ locales: { "../x": { title: "x" } } }, SITE)).toThrow(AiConfigError);
    expect(() => checkAiConfig({ locales: { ja: { title: " " } } }, SITE)).toThrow(AiConfigError);
    expect(() => checkAiConfig({ locales: { ja: { title: "x" } }, robotsTxt: { addCrawlers: ["Bad Bot\nDisallow: /"] } }, SITE)).toThrow(AiConfigError);
  });
});

describe("markdownPath", () => {
  it("記事のURLに .md を付ける（末尾の / は除く）。別のオリジンやトップは対象外", () => {
    expect(markdownPath(`${SITE}/ja/blog/a`, SITE)).toBe("/ja/blog/a.md");
    expect(markdownPath(`${SITE}/ja/blog/a/`, SITE)).toBe("/ja/blog/a.md");
    expect(markdownPath("https://other.example/ja/blog/a", SITE)).toBeNull();
    expect(markdownPath(`${SITE}/`, SITE)).toBeNull();
    expect(markdownPath(`${SITE}/a?x=1`, SITE)).toBeNull();
    expect(articlePath("not a url", SITE)).toBeNull();
  });
});

describe("toLlmsTxt", () => {
  it("ロケールごとに、記事のMarkdown版へのリンクを並べ、文字をエスケープする", () => {
    const txt = toLlmsTxt(config, {
      ja: [item(`${SITE}/ja/blog/a`, "A [x](javascript:alert(1))"), item("https://other.example/b"), item(`${SITE}/ja/blog/(c)`, null)],
      en: [item(`${SITE}/en/blog/d`, "D")],
    });
    expect(txt.startsWith("# 例のブログ\n\n> 技術の記事\n\n## 例のブログ\n")).toBe(true);
    expect(txt).toContain("- [A \\[x\\](javascript:alert(1))](https://example.com/ja/blog/a.md)");
    expect(txt).toContain("(https://example.com/ja/blog/%28c%29.md)");
    expect(txt).not.toContain("other.example");
    expect(txt).toContain("## English articles\n\n- [D](https://example.com/en/blog/d.md)");
  });
});

describe("toRobotsTxt", () => {
  it("学習用のクローラーに、拒否した記事とそのMarkdown版を Disallow で並べ、サイトの行を後ろに足す", () => {
    const txt = toRobotsTxt(config, [`${SITE}/ja/blog/a`, `${SITE}/ja/blog/*x$`, "https://other.example/z"]);
    for (const c of config.robotsTxt!.crawlers) expect(txt).toContain(`User-agent: ${c}\n`);
    expect(txt).toContain("Disallow: /ja/blog/a\n");
    expect(txt).toContain("Disallow: /ja/blog/a.md\n");
    expect(txt).toContain("Disallow: /ja/blog/%2Ax%24\n");
    expect(txt).not.toContain("/z");
    expect(txt.trimEnd().endsWith("User-agent: *\nDisallow: /admin")).toBe(true);
  });

  it("拒否した記事がなければ、空の Disallow にする", () => {
    const txt = toRobotsTxt(checkAiConfig({ locales: { ja: { title: "x" } }, robotsTxt: {} }, SITE), []);
    expect(txt).toContain(`User-agent: ${DEFAULT_AI_TRAINING_CRAWLERS[0]}\n`);
    expect(txt).toContain("Disallow:\n");
  });
});
