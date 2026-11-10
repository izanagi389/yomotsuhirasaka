import { describe, expect, it, vi } from "vitest";

import { containsSecret } from "../src/module.ts";
import { RegisterError, checkUrl, registerQueries } from "../src/register.ts";
import { QueryError, fetchQuery, parseVariables } from "../src/runtime/server/fetch-query.ts";
import { readLimitedText } from "../src/runtime/server/read-body.ts";
import { ANSWER_QUERY, CORRECTIONS_QUERY, SEARCH_QUERY, isModuleOnlyQuery } from "../src/runtime/server/names.ts";

const target = { url: "https://yomi.example.com", workspaceId: "ws-1", readKey: "yomi_key_read" };
const reply = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

describe("readLimitedText", () => {
  const streamOf = (...parts: string[]) =>
    new ReadableStream<Uint8Array>({
      start(c) {
        for (const p of parts) c.enqueue(new TextEncoder().encode(p));
        c.close();
      },
    });
  it("上限まで読み、超えたら null を返す", async () => {
    expect(await readLimitedText(streamOf("ab", "cd"), 4)).toBe("abcd");
    expect(await readLimitedText(streamOf("ab", "cde"), 4)).toBeNull();
    expect(await readLimitedText(undefined, 4)).toBe("");
  });
});

describe("isModuleOnlyQuery", () => {
  it("検索と回答は汎用のルートから呼べず、サイトのクエリと訂正の一覧は呼べる", () => {
    expect(isModuleOnlyQuery(SEARCH_QUERY)).toBe(true);
    expect(isModuleOnlyQuery(ANSWER_QUERY)).toBe(true);
    expect(isModuleOnlyQuery(CORRECTIONS_QUERY)).toBe(false);
    expect(isModuleOnlyQuery("postBySlug")).toBe(false);
  });
});

describe("fetchQuery", () => {
  it("登録したIDと変数で GET し、キーは Authorization だけで送る", async () => {
    const f = vi.fn(async (_u: URL | string, _i?: RequestInit) => reply(200, { data: { postList: [] } }));
    expect(await fetchQuery(target, "q1", { locale: "ja" }, f as unknown as typeof fetch)).toEqual({ postList: [] });
    const [u, init] = f.mock.calls[0]!;
    expect(String(u)).toBe("https://yomi.example.com/ws/ws-1/graphql?id=q1&variables=%7B%22locale%22%3A%22ja%22%7D");
    expect(String(u)).not.toContain("yomi_key");
    expect((init?.headers as Record<string, string>)["Authorization"]).toBe("Bearer yomi_key_read");
    expect(init?.redirect).toBe("error");
  });

  it("POST では、IDと変数を本文で送り、URL には載せない（検索語）", async () => {
    const f = vi.fn(async (_u: URL | string, _i?: RequestInit) => reply(200, { data: { search: { hits: [] } } }));
    expect(await fetchQuery(target, "q1", { text: "トマト" }, f as unknown as typeof fetch, "POST")).toEqual({ search: { hits: [] } });
    const [u, init] = f.mock.calls[0]!;
    expect(String(u)).toBe("https://yomi.example.com/ws/ws-1/graphql");
    expect(init?.method).toBe("POST");
    expect(JSON.parse(String(init?.body))).toEqual({ id: "q1", variables: { text: "トマト" } });
    expect((init?.headers as Record<string, string>)["Content-Type"]).toBe("application/json");
    expect((init?.headers as Record<string, string>)["Authorization"]).toBe("Bearer yomi_key_read");
  });

  it("CMSの拒否は、理由のコードだけを添えて 502 にする", async () => {
    const f = async () => reply(401, { error: "unauthenticated", message: "x" });
    await expect(fetchQuery(target, "q1", undefined, f as unknown as typeof fetch)).rejects.toMatchObject({ status: 502 });
    const g = async () => reply(200, { errors: [{ message: "bad" }] });
    await expect(fetchQuery(target, "q1", undefined, g as unknown as typeof fetch)).rejects.toBeInstanceOf(QueryError);
  });

  it("変数は JSON のオブジェクトだけ、大きさに上限がある", () => {
    expect(parseVariables(undefined)).toBeUndefined();
    expect(parseVariables('{"slug":"a"}')).toEqual({ slug: "a" });
    for (const bad of ["[1]", "1", "{", "x".repeat(5000), 3]) expect(() => parseVariables(bad)).toThrow(QueryError);
  });
});

describe("registerQueries", () => {
  it("register のキーで登録し、名前 → ID を返す", async () => {
    const f = vi.fn(async () => reply(201, { id: "abc" }));
    const ids = await registerQueries({ url: "https://yomi.example.com", workspaceId: "ws-1", registerKey: "yomi_key_reg" }, { postList: "{ postList { id } }" }, f as unknown as typeof fetch);
    expect(ids).toEqual({ postList: "abc" });
  });

  it("登録できなければ、理由を付けて止める", async () => {
    const f = async () => reply(422, { error: "invalid_query", message: "Query is too deep" });
    await expect(registerQueries({ url: "https://yomi.example.com", workspaceId: "w", registerKey: "k" }, { q: "{}" }, f as unknown as typeof fetch)).rejects.toThrow(/Query is too deep/);
  });

  it("接続先は https だけ（手元だけ http）", () => {
    expect(checkUrl("https://yomi.example.com/")).toBe("https://yomi.example.com");
    expect(checkUrl("http://localhost:8080")).toBe("http://localhost:8080");
    expect(() => checkUrl("http://yomi.example.com")).toThrow(RegisterError);
    expect(() => checkUrl("https://u:p@yomi.example.com")).toThrow(RegisterError);
  });
});

describe("containsSecret", () => {
  it("公開の設定に入ったキーやトークンを見つける", () => {
    expect(containsSecret({ yomi: { routePrefix: "/_yomi/q" } })).toBe(false);
    expect(containsSecret({ yomi: { readKey: "yomi_key_abc" } })).toBe(true);
    expect(containsSecret({ a: ["x", { b: "yomi_pat_zzz" }] })).toBe(true);
  });
});

describe("Webhookの署名の検証", async () => {
  const { createHmac } = await import("node:crypto");
  const { SeenEvents, parseSignature, verifySignature, MAX_SKEW_SECONDS } = await import("../src/runtime/server/webhook-verify.ts");
  const now = 1_800_000_000;
  const body = JSON.stringify({ type: "entry.published", workspaceId: "w", entryId: "e" });
  const sign = (secret: string, t = now, id = "evt-1", b = body) => createHmac("sha256", secret).update(`${t}.${id}.${b}`).digest("hex");
  const header = (sigs: string[], t = now, id = "evt-1") => [`t=${t}`, `id=${id}`, ...sigs.map((s) => `v1=${s}`)].join(",");

  it("正しい署名を受け付け、同じイベントの再送は受け付けない", () => {
    const seen = new SeenEvents();
    expect(verifySignature(header([sign("s1")]), body, ["s1"], seen, now)).toBe(true);
    expect(verifySignature(header([sign("s1")]), body, ["s1"], seen, now)).toBe(false);
  });

  it("秘密の入れ替え中は、新旧のどちらの署名でも受け付ける", () => {
    expect(verifySignature(header([sign("old", now, "evt-2"), sign("new", now, "evt-2")], now, "evt-2"), body, ["new"], new SeenEvents(), now)).toBe(true);
    expect(verifySignature(header([sign("old", now, "evt-3")], now, "evt-3"), body, ["new", "old"], new SeenEvents(), now)).toBe(true);
  });

  it("秘密・本文・時刻・イベントIDのどれかが違えば受け付けない", () => {
    const seen = new SeenEvents();
    expect(verifySignature(header([sign("other")]), body, ["s1"], seen, now)).toBe(false);
    expect(verifySignature(header([sign("s1")]), body + " ", ["s1"], seen, now)).toBe(false);
    expect(verifySignature(header([sign("s1", now - MAX_SKEW_SECONDS - 1)], now - MAX_SKEW_SECONDS - 1), body, ["s1"], seen, now)).toBe(false);
    expect(verifySignature(header([sign("s1", now, "evt-x")], now, "evt-y"), body, ["s1"], seen, now)).toBe(false);
    expect(verifySignature(header([sign("s1")]), body, [], seen, now)).toBe(false);
    expect(verifySignature(undefined, body, ["s1"], seen, now)).toBe(false);
    // 偽の署名では、イベントIDを覚えない（正しいイベントを締め出させない）
    expect(verifySignature(header([sign("s1")]), body, ["s1"], seen, now)).toBe(true);
  });

  it("ヘッダーの形が違えば読まない", () => {
    for (const h of ["", "t=1", "t=1,id=a", "t=x,id=a,v1=" + "0".repeat(64), "t=1,id=a,v1=zz", "t=1,id=a,v2=" + "0".repeat(64)]) {
      expect(parseSignature(h), h).toBeNull();
    }
  });
});

describe("リダイレクトの規則", async () => {
  const { normalizePath, toPathRules, toRouteRules } = await import("../src/runtime/server/redirect-rules.ts");
  const rules = toPathRules(
    [
      { from: "https://example.com/ja/blog/old/", to: "https://example.com/ja/blog/new", status: 301 },
      { from: "https://example.com/ja/blog/gone", to: null, status: 410 },
      { from: "https://example.com/en/x", to: "https://other.example/en/x", status: 301 },
      { from: "https://other.example/en/y", to: "https://other.example/en/z", status: 301 },
      { from: "https://example.com/bad", to: "javascript:alert(1)", status: 301 },
    ],
    "https://example.com",
  );

  it("サイトのオリジンのURLだけを、パスの規則にする", () => {
    expect(rules.get("/ja/blog/old")).toEqual({ status: 301, to: "/ja/blog/new" });
    expect(rules.get("/ja/blog/gone")).toEqual({ status: 410, to: null });
    expect(rules.get("/en/x")).toEqual({ status: 301, to: "https://other.example/en/x" });
    expect(rules.has("/en/y")).toBe(false);
    expect(rules.has("/bad")).toBe(false);
    expect(normalizePath("/a/")).toBe("/a");
    expect(normalizePath("/")).toBe("/");
  });

  it("サイト自身の固定のページとモジュールのルートには、CMS のリダイレクトを当てない（R26）", async () => {
    const { staticPagePaths, withoutProtected } = await import("../src/runtime/server/redirect-rules.ts");
    const pages = [
      { path: "/" },
      { path: "/contact/" },
      { path: "/ja/blog/:slug()" },
      { path: "/about", children: [{ path: "team" }, { path: ":id" }] },
      { path: "/[...all]" },
    ];
    expect(staticPagePaths(pages)).toEqual(["/", "/contact", "/about", "/about/team"]);
    const all = toPathRules(
      [
        { from: "https://example.com/contact", to: "https://example.com/ja/blog/a", status: 301 },
        { from: "https://example.com/about/team?x=1", to: "https://example.com/ja/blog/a", status: 301 },
        { from: "https://example.com/_yomi/webhook", to: null, status: 410 },
        { from: "https://example.com/ja/blog/old", to: "https://example.com/ja/blog/new", status: 301 },
      ],
      "https://example.com",
    );
    const kept = withoutProtected(all, new Set([...staticPagePaths(pages), "/_yomi/webhook"]));
    expect([...kept.keys()]).toEqual(["/ja/blog/old"]);
  });

  it("routeRules には 301 だけを入れる", () => {
    expect(toRouteRules(rules)).toEqual({
      "/ja/blog/old": { redirect: { to: "/ja/blog/new", statusCode: 301 } },
      "/en/x": { redirect: { to: "https://other.example/en/x", statusCode: 301 } },
    });
  });
});
