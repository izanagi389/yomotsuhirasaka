// 2つの応答が「区別できない」ことを確かめる（アカウントの有無などを推測させないための約束で使う）。

import { expect } from "vitest";
import type { HttpResponse } from "./http.js";

/** 毎回変わるのが当然のヘッダー。比べない。 */
const VOLATILE_HEADERS = new Set(["date", "x-request-id", "request-id", "traceparent", "server-timing", "set-cookie", "content-length", "etag", "age"]);

/** 本文から、毎回変わるのが当然の値（ID、時刻、トークンらしき長い値）を伏せる。 */
export function normalizeBody(text: string): string {
  return text
    .replace(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/gi, "<uuid>")
    .replace(/\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?/g, "<time>")
    .replace(/\b[0-9A-Za-z_-]{24,}\b/g, "<opaque>");
}

/** Set-Cookie の名前と属性（値は除く）。 */
function cookieShapes(res: HttpResponse): string[] {
  return res.setCookies
    .map((c) => {
      const [pair = "", ...attrs] = c.split(";").map((s) => s.trim().toLowerCase());
      const name = pair.split("=")[0];
      const attrNames = attrs.map((a) => a.split("=")[0]).sort();
      return `${name}|${attrNames.join(",")}`;
    })
    .sort();
}

function headerNames(res: HttpResponse): string[] {
  return [...res.headers.keys()].map((k) => k.toLowerCase()).filter((k) => !VOLATILE_HEADERS.has(k)).sort();
}

function stableHeaderValues(res: HttpResponse): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of res.headers.entries()) {
    const key = k.toLowerCase();
    if (!VOLATILE_HEADERS.has(key)) out[key] = v;
  }
  return out;
}

export function expectIndistinguishable(a: HttpResponse, b: HttpResponse, what: string): void {
  expect(a.status, `${what}: 状態コードが違う`).toBe(b.status);
  expect(normalizeBody(a.text), `${what}: 本文が違う`).toBe(normalizeBody(b.text));
  expect(headerNames(a), `${what}: ヘッダーの種類が違う`).toEqual(headerNames(b));
  expect(stableHeaderValues(a), `${what}: ヘッダーの値が違う`).toEqual(stableHeaderValues(b));
  expect(cookieShapes(a), `${what}: Set-Cookie の種類が違う`).toEqual(cookieShapes(b));
}
