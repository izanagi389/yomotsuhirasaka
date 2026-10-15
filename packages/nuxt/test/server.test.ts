import { describe, expect, it, vi } from "vitest";

import { containsSecret } from "../src/module.ts";
import { RegisterError, checkUrl, registerQueries } from "../src/register.ts";
import { QueryError, fetchQuery, parseVariables } from "../src/runtime/server/fetch-query.ts";

const target = { url: "https://cms.example.com", workspaceId: "ws-1", readKey: "cms_key_read" };
const reply = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

describe("fetchQuery", () => {
  it("登録したIDと変数で GET し、キーは Authorization だけで送る", async () => {
    const f = vi.fn(async (_u: URL | string, _i?: RequestInit) => reply(200, { data: { postList: [] } }));
    expect(await fetchQuery(target, "q1", { locale: "ja" }, f as unknown as typeof fetch)).toEqual({ postList: [] });
    const [u, init] = f.mock.calls[0]!;
    expect(String(u)).toBe("https://cms.example.com/ws/ws-1/graphql?id=q1&variables=%7B%22locale%22%3A%22ja%22%7D");
    expect(String(u)).not.toContain("cms_key");
    expect((init?.headers as Record<string, string>)["Authorization"]).toBe("Bearer cms_key_read");
    expect(init?.redirect).toBe("error");
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
    const ids = await registerQueries({ url: "https://cms.example.com", workspaceId: "ws-1", registerKey: "cms_key_reg" }, { postList: "{ postList { id } }" }, f as unknown as typeof fetch);
    expect(ids).toEqual({ postList: "abc" });
  });

  it("登録できなければ、理由を付けて止める", async () => {
    const f = async () => reply(422, { error: "invalid_query", message: "Query is too deep" });
    await expect(registerQueries({ url: "https://cms.example.com", workspaceId: "w", registerKey: "k" }, { q: "{}" }, f as unknown as typeof fetch)).rejects.toThrow(/Query is too deep/);
  });

  it("接続先は https だけ（手元だけ http）", () => {
    expect(checkUrl("https://cms.example.com/")).toBe("https://cms.example.com");
    expect(checkUrl("http://localhost:8080")).toBe("http://localhost:8080");
    expect(() => checkUrl("http://cms.example.com")).toThrow(RegisterError);
    expect(() => checkUrl("https://u:p@cms.example.com")).toThrow(RegisterError);
  });
});

describe("containsSecret", () => {
  it("公開の設定に入ったキーやトークンを見つける", () => {
    expect(containsSecret({ cms: { routePrefix: "/_cms/q" } })).toBe(false);
    expect(containsSecret({ cms: { readKey: "cms_key_abc" } })).toBe(true);
    expect(containsSecret({ a: ["x", { b: "cms_pat_zzz" }] })).toBe(true);
  });
});
