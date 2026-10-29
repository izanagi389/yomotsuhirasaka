import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { effectScope, h, nextTick } from "vue";
import { renderToString } from "vue/server-renderer";

import { cmsHighlight, type CmsSearchHit, type CmsSearchResult } from "../src/runtime/search.ts";
import { QueryError } from "../src/runtime/server/fetch-query.ts";
import { EMPTY_RESULT, RateLimiter, cachedSearch, clearSearchCache, clientIp, parseSearchParams } from "../src/runtime/server/search-core.ts";

vi.mock("nuxt/app", () => ({
  useRuntimeConfig: () => ({ public: { cms: { searchRoute: "/_cms/search" } } }),
  useFetch: vi.fn(),
  useHead: vi.fn(),
}));

const { useCmsSearch } = await import("../src/runtime/composables.ts");
const { default: CmsSearchBox, hitHref } = await import("../src/runtime/search-box.ts");

const hit = (over: Partial<CmsSearchHit> = {}): CmsSearchHit => ({
  entryId: "e-1",
  locale: "ja",
  url: "/posts/a",
  anchor: "h-1",
  breadcrumb: ["記事A", "見出し"],
  excerpt: "春の庭でトマトを育てる",
  matches: [],
  score: 1,
  ...over,
});

describe("cmsHighlight", () => {
  it("一致した箇所とそれ以外に区切る（UTF-16 の位置）", () => {
    expect(cmsHighlight("春の庭でトマトを育てる", [{ start: 4, length: 3 }])).toEqual([
      { text: "春の庭で", match: false },
      { text: "トマト", match: true },
      { text: "を育てる", match: false },
    ]);
    // サロゲートペア（絵文字）も UTF-16 の単位で数える
    expect(cmsHighlight("🍅トマト", [{ start: 2, length: 3 }])).toEqual([
      { text: "🍅", match: false },
      { text: "トマト", match: true },
    ]);
  });

  it("範囲の外・重なり・逆順・長さ 0 の一致は、使える部分だけを使う", () => {
    // 0〜2、（重なりを除いて）2〜3、3〜末尾（範囲の外は切る）が一致。続いた一致は1つにまとめる
    expect(cmsHighlight("abcdefgh", [{ start: 3, length: 3 }, { start: 0, length: 2 }, { start: 1, length: 2 }, { start: 7, length: 0 }])).toEqual([
      { text: "abcdef", match: true },
      { text: "gh", match: false },
    ]);
    expect(cmsHighlight("abc", [{ start: 2, length: 10 }])).toEqual([
      { text: "ab", match: false },
      { text: "c", match: true },
    ]);
    expect(cmsHighlight("abc", null)).toEqual([{ text: "abc", match: false }]);
    expect(cmsHighlight("", [{ start: 0, length: 1 }])).toEqual([]);
  });
});

describe("parseSearchParams", () => {
  it("既定は HYBRID で 10 件。検索語は前後の空白を除く", () => {
    expect(parseSearchParams({ q: "  トマト ", locale: "ja" })).toEqual({ q: "トマト", locale: "ja", mode: "HYBRID", first: 10 });
    expect(parseSearchParams({ q: "x", locale: "en-US", mode: "KEYWORD", first: "50" })).toEqual({ q: "x", locale: "en-US", mode: "KEYWORD", first: 50 });
  });

  it("長すぎる検索語・ロケールなし・知らないモード・範囲外の件数・同じ名前の繰り返しは 400", () => {
    for (const bad of [
      { q: "x".repeat(501), locale: "ja" },
      { q: "x" },
      { q: "x", locale: "../ja" },
      { q: "x", locale: "ja", mode: "FUZZY" },
      { q: "x", locale: "ja", first: "0" },
      { q: "x", locale: "ja", first: "51" },
      { q: "x", locale: "ja", first: "1.5" },
      { q: ["a", "b"], locale: "ja" },
    ]) {
      expect(() => parseSearchParams(bad), JSON.stringify(bad)).toThrow(QueryError);
    }
    expect(parseSearchParams({ q: "x".repeat(500), locale: "ja" }).q).toHaveLength(500);
  });
});

describe("clientIp", () => {
  it("ヘッダーを設定していなければ、接続元のアドレスを使う（訪問者が偽ったヘッダーは見ない）", () => {
    expect(clientIp({ "x-forwarded-for": "1.1.1.1" }, "10.0.0.5", "")).toBe("10.0.0.5");
  });

  it("設定したヘッダーの、いちばん右（手前のプロキシが付けた値）を使う", () => {
    expect(clientIp({ "x-forwarded-for": "6.6.6.6, 203.0.113.7" }, "10.0.0.5", "X-Forwarded-For")).toBe("203.0.113.7");
    expect(clientIp({ "cf-connecting-ip": "198.51.100.2" }, "10.0.0.5", "cf-connecting-ip")).toBe("198.51.100.2");
    expect(clientIp({}, "10.0.0.5", "cf-connecting-ip")).toBe("10.0.0.5");
  });
});

describe("RateLimiter", () => {
  it("1分ごとに、上限の回数まで通す（訪問者ごと）", () => {
    const l = new RateLimiter(2);
    const t = 60_000 * 100;
    expect([l.take("a", t), l.take("a", t + 1), l.take("a", t + 2), l.take("b", t + 3)]).toEqual([true, true, false, true]);
    expect(l.take("a", t + 60_000)).toBe(true);
  });
});

describe("cachedSearch", () => {
  const params = { q: "トマト", locale: "ja", mode: "HYBRID" as const, first: 10 };
  const result: CmsSearchResult = { vectorSearch: "AVAILABLE", keywordSearch: "DISABLED", hits: [hit()] };
  beforeEach(() => clearSearchCache());

  it("同じ検索は 60 秒だけ覚えておき、Webhook を受けたら（clearSearchCache）取り直す", async () => {
    const load = vi.fn(async () => result);
    await cachedSearch(params, load, 0);
    await cachedSearch(params, load, 59_000);
    expect(load).toHaveBeenCalledTimes(1);
    await cachedSearch({ ...params, locale: "en" }, load, 59_000);
    expect(load).toHaveBeenCalledTimes(2);
    await cachedSearch(params, load, 61_000);
    expect(load).toHaveBeenCalledTimes(3);
    clearSearchCache();
    await cachedSearch(params, load, 61_001);
    expect(load).toHaveBeenCalledTimes(4);
  });

  it("失敗は覚えない", async () => {
    await expect(cachedSearch(params, async () => Promise.reject(new Error("down")), 0)).rejects.toThrow("down");
    expect(await cachedSearch(params, async () => EMPTY_RESULT, 1)).toEqual(EMPTY_RESULT);
  });
});

describe("hitHref", () => {
  it("記事のURLに、当たったセクションのアンカーを付ける", () => {
    expect(hitHref(hit())).toBe("/posts/a#h-1");
    expect(hitHref(hit({ anchor: null }))).toBe("/posts/a");
  });

  it("URLがない・描いてよいURLでないときはリンクにしない", () => {
    expect(hitHref(hit({ url: null }))).toBeNull();
    expect(hitHref(hit({ url: "javascript:alert(1)" }))).toBeNull();
    expect(hitHref(hit({ url: "//evil.example/x" }))).toBeNull();
  });
});

describe("useCmsSearch", () => {
  const original = globalThis.window;
  beforeEach(() => {
    vi.useFakeTimers();
    (globalThis as { window?: unknown }).window = {};
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
    (globalThis as { window?: unknown }).window = original;
  });

  it("入力が止まってから検索し、古い検索の結果では上書きしない", async () => {
    const replies: ((r: Response) => void)[] = [];
    const urls: string[] = [];
    vi.stubGlobal("fetch", (u: string) => {
      urls.push(u);
      return new Promise<Response>((resolve) => replies.push(resolve));
    });
    const scope = effectScope();
    const s = scope.run(() => useCmsSearch({ locale: "ja", debounceMs: 100 }))!;
    s.text.value = "ト";
    await nextTick();
    s.text.value = "トマ";
    await nextTick();
    await vi.advanceTimersByTimeAsync(100);
    expect(urls).toEqual(["/_cms/search?q=%E3%83%88%E3%83%9E&locale=ja"]);
    s.text.value = "トマト";
    await nextTick();
    await vi.advanceTimersByTimeAsync(100);
    expect(urls).toHaveLength(2);
    const body = (excerpt: string) => new Response(JSON.stringify({ ...EMPTY_RESULT, hits: [hit({ excerpt })] }), { status: 200 });
    // 新しい検索の結果が先に届き、古い検索の結果が後から届く
    replies[1]!(body("新しい"));
    await vi.advanceTimersByTimeAsync(0);
    replies[0]!(body("古い"));
    await vi.advanceTimersByTimeAsync(0);
    expect(s.status.value).toBe("done");
    expect(s.result.value?.hits[0]?.excerpt).toBe("新しい");
    scope.stop();
  });

  it("回数の制限（429）は rateLimited、空の入力は検索しない", async () => {
    const fetchMock = vi.fn(async (_u: string) => new Response("", { status: 429 }));
    vi.stubGlobal("fetch", fetchMock);
    const scope = effectScope();
    const s = scope.run(() => useCmsSearch({ locale: "ja", mode: "KEYWORD", first: 5, debounceMs: 10 }))!;
    s.text.value = "   ";
    await nextTick();
    await vi.advanceTimersByTimeAsync(10);
    expect(fetchMock).not.toHaveBeenCalled();
    expect(s.status.value).toBe("idle");
    s.text.value = "トマト";
    await nextTick();
    await vi.advanceTimersByTimeAsync(10);
    expect(fetchMock.mock.calls[0]![0]).toBe("/_cms/search?q=%E3%83%88%E3%83%9E%E3%83%88&locale=ja&mode=KEYWORD&first=5");
    expect(s.status.value).toBe("rateLimited");
    scope.stop();
  });
});

describe("CmsSearchBox", () => {
  it("ラベル付きの検索欄だけを描き、クラスも決まった文言も持たない", async () => {
    const html = await renderToString(h(CmsSearchBox, { label: "サイト内を検索", locale: "ja" }));
    expect(html).toMatch(/^<search><label for="([^"]+)">サイト内を検索<\/label><input id="\1" type="search" value maxlength="500" autocomplete="off"><div role="status" aria-live="polite"><\/div><!----><\/search>$/);
    expect(html).not.toContain("class=");
  });
});
