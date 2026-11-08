import { describe, expect, it, vi } from "vitest";

import { containsSecret } from "../src/module.ts";
import { fetchQuery } from "../src/runtime/server/fetch-query.ts";
import { checkPreviewToken, cookieSeconds, redirectPath } from "../src/runtime/server/preview-check.ts";

const target = { url: "https://yomi.example.com", workspaceId: "ws-1", readKey: "yomi_key_read" };
const reply = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

describe("checkPreviewToken", () => {
  it("キーとトークンをヘッダーで送り、有効なら記事の情報を返す", async () => {
    const f = vi.fn(async (_u: URL | string, _i?: RequestInit) => reply(200, { entryId: "e1", url: "https://site.example.com/ja/a", expiresAt: "2026-01-01T00:10:00Z" }));
    expect(await checkPreviewToken(target, "yomi_prv_abc", f as unknown as typeof fetch)).toEqual({ entryId: "e1", url: "https://site.example.com/ja/a", expiresAt: "2026-01-01T00:10:00Z" });
    const [u, init] = f.mock.calls[0]!;
    expect(String(u)).toBe("https://yomi.example.com/ws/ws-1/preview");
    expect(String(u)).not.toContain("yomi_prv");
    const headers = init?.headers as Record<string, string>;
    expect(headers["Authorization"]).toBe("Bearer yomi_key_read");
    expect(headers["X-Yomi-Preview-Token"]).toBe("yomi_prv_abc");
  });

  it("形の違うトークン・CMSの拒否・届かないときは null", async () => {
    const never = vi.fn();
    expect(await checkPreviewToken(target, "yomi_key_x", never as unknown as typeof fetch)).toBeNull();
    expect(never).not.toHaveBeenCalled();
    expect(await checkPreviewToken(target, "yomi_prv_x", (async () => reply(401, { error: "invalid_preview_token" })) as unknown as typeof fetch)).toBeNull();
    expect(await checkPreviewToken(target, "yomi_prv_x", (async () => { throw new Error("down"); }) as unknown as typeof fetch)).toBeNull();
  });
});

describe("redirectPath", () => {
  it("記事のURLのパスとクエリだけにする（別のサイトへは飛ばさない）", () => {
    expect(redirectPath("https://site.example.com/ja/blog/a?x=1#h")).toBe("/ja/blog/a?x=1");
    expect(redirectPath("https://evil.example.com//evil.example/x")).toBe("/");
    expect(redirectPath(null)).toBe("/");
    expect(redirectPath("not a url")).toBe("/");
  });
});

describe("cookieSeconds", () => {
  it("有効期限までの秒数（0〜600）", () => {
    const now = Date.parse("2026-01-01T00:00:00Z");
    expect(cookieSeconds("2026-01-01T00:05:00Z", now)).toBe(300);
    expect(cookieSeconds("2026-01-01T01:00:00Z", now)).toBe(600);
    expect(cookieSeconds("2025-12-31T23:00:00Z", now)).toBe(0);
    expect(cookieSeconds("bad", now)).toBe(0);
  });
});

describe("fetchQuery のプレビュー", () => {
  it("トークンがあればヘッダーで送り、トークンの拒否はコードを添える", async () => {
    const f = vi.fn(async (_u: URL | string, _i?: RequestInit) => reply(200, { data: {} }));
    await fetchQuery({ ...target, previewToken: "yomi_prv_t" }, "q1", undefined, f as unknown as typeof fetch);
    expect((f.mock.calls[0]![1]?.headers as Record<string, string>)["X-Yomi-Preview-Token"]).toBe("yomi_prv_t");
    const g = async () => reply(401, { error: "invalid_preview_token" });
    await expect(fetchQuery({ ...target, previewToken: "yomi_prv_t" }, "q1", undefined, g as unknown as typeof fetch)).rejects.toMatchObject({ status: 502, code: "invalid_preview_token" });
  });

  it("公開の設定にプレビュー用トークンがあれば止める", () => {
    expect(containsSecret({ yomi: { token: "yomi_prv_x" } })).toBe(true);
  });
});
