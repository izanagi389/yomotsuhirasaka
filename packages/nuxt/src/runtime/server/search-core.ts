// サイトのサーバーの、サイト内検索のルートの中身（Nitro に依存しない。テストで直接呼ぶ）。
// - 訪問者の入力のたびに呼ばれ、ベクトル検索ならワークスペースの埋め込みの予算も減るので、訪問者ごとに回数を制限する。
//   CMSの配信APIの回数制限はAPIキーごとで、サイトの訪問者全員で共有になるため
// - 数えるのはサイトのサーバーのメモリの中だけ（複数のプロセスで動かすと、プロセスごとに数える）
// - 同じ検索（検索語・ロケール・モード・件数）の結果を、短い時間だけ覚えておく。CMSのWebhookを受けたら消す

import { isIPv6 } from "node:net";

import { DEFAULT_FIRST, MAX_FIRST, MAX_QUERY_CHARS, SEARCH_MODES, type YomiSearchMode, type YomiSearchResult } from "../search.ts";
import { QueryError } from "./fetch-query.ts";

/** 1分あたりの回数の既定値。 */
export const DEFAULT_PER_MINUTE = 30;
/** 結果を覚えておく時間（ミリ秒）と、覚えておく数の上限。 */
export const RESULT_TTL_MS = 60 * 1000;
const MAX_CACHED = 500;
/** ロケールの形（BCP 47 のおおまかな形）。 */
const LOCALE = /^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$/;

export interface SearchParams {
  q: string;
  locale: string;
  mode: YomiSearchMode;
  first: number;
}

/** 検索の条件（`q`・`locale`・`mode`・`first`。値は文字列）を確かめる。 */
export function parseSearchParams(query: Record<string, unknown>): SearchParams {
  const one = (k: string): string | undefined => {
    const v = query[k];
    if (v === undefined) return undefined;
    if (typeof v !== "string") throw new QueryError(400, `${k} must be given once`);
    return v;
  };
  const q = (one("q") ?? "").trim();
  if (q.length > MAX_QUERY_CHARS) throw new QueryError(400, `q is longer than ${MAX_QUERY_CHARS} characters`);
  const locale = one("locale") ?? "";
  if (!LOCALE.test(locale)) throw new QueryError(400, "locale is required");
  const mode = (one("mode") ?? "HYBRID") as YomiSearchMode;
  if (!SEARCH_MODES.includes(mode)) throw new QueryError(400, "mode must be HYBRID, VECTOR or KEYWORD");
  const rawFirst = one("first");
  const first = rawFirst === undefined ? DEFAULT_FIRST : Number(rawFirst);
  if (!Number.isInteger(first) || first < 1 || first > MAX_FIRST) throw new QueryError(400, `first must be 1 to ${MAX_FIRST}`);
  return { q, locale, mode, first };
}

/** 検索のリクエストの本文の上限（バイト）。検索語の上限より十分に大きく、大きな本文は読まない。 */
export const MAX_SEARCH_BODY_BYTES = 8 * 1024;

/**
 * ルートの本文（JSON の `{ q, locale, mode?, first? }`）を確かめる。検索語は URL に載せない
 * （プロキシやアクセスログに残さない）ので、ルートは POST で本文を受け取る。
 */
export function parseSearchBody(text: string | null): SearchParams {
  if (text === null) throw new QueryError(413, "the search request is too large");
  let v: unknown;
  try {
    v = JSON.parse(text);
  } catch {
    throw new QueryError(400, "send the search as a JSON object");
  }
  if (typeof v !== "object" || v === null || Array.isArray(v)) throw new QueryError(400, "send the search as a JSON object");
  const body = v as Record<string, unknown>;
  const fields: Record<string, unknown> = {};
  for (const k of ["q", "locale", "mode"] as const) {
    if (body[k] !== undefined && typeof body[k] !== "string") throw new QueryError(400, `${k} must be a string`);
    fields[k] = body[k];
  }
  if (body["first"] !== undefined) {
    if (typeof body["first"] !== "number") throw new QueryError(400, "first must be a number");
    fields["first"] = String(body["first"]);
  }
  return parseSearchParams(fields);
}

/** 空の検索語の結果（CMSを呼ばない）。 */
export const EMPTY_RESULT: YomiSearchResult = { vectorSearch: "AVAILABLE", keywordSearch: "AVAILABLE", hits: [] };

/**
 * 訪問者のIPアドレス。`ipHeader` を設定していなければ接続元のアドレス。
 * `x-forwarded-for` は訪問者が偽れるので、いちばん右（サイトの手前のプロキシが付けた値）を使う。
 */
export function clientIp(headers: Record<string, string | undefined>, socketIp: string | undefined, ipHeader: string): string {
  if (ipHeader) {
    const raw = headers[ipHeader.toLowerCase()];
    if (raw) {
      const parts = raw.split(",").map((s) => s.trim()).filter(Boolean);
      const value = parts[parts.length - 1];
      if (value) return limitKey(value);
    }
  }
  return socketIp ? limitKey(socketIp) : "unknown";
}

/**
 * 回数を数える単位。IPv6 は /64 にまとめる（1つの回線が /64 を持つので、アドレスを変えて制限を逃れられないように）。
 * IPv4射影アドレスは IPv4 に直す。IPアドレスの形でなければ、そのまま返す。
 */
export function limitKey(ip: string): string {
  const bare = ip.replace(/%.*$/, "");
  if (!isIPv6(bare)) return ip;
  const hostname = new URL(`http://[${bare}]/`).hostname.slice(1, -1);
  const [head = "", tail = ""] = hostname.split("::");
  const left = head === "" ? [] : head.split(":");
  const right = tail === "" ? [] : tail.split(":");
  const groups = hostname.includes("::") ? [...left, ...Array<string>(8 - left.length - right.length).fill("0"), ...right] : left;
  const mapped = groups.slice(0, 5).every((g) => g === "0") && groups[5] === "ffff";
  if (mapped) {
    const [hi = 0, lo = 0] = [Number.parseInt(groups[6] ?? "0", 16), Number.parseInt(groups[7] ?? "0", 16)];
    return `${hi >> 8}.${hi & 255}.${lo >> 8}.${lo & 255}`;
  }
  return `${groups.slice(0, 4).join(":")}::/64`;
}

/** 1分ごとの回数の制限（固定の窓）。 */
export class RateLimiter {
  private readonly counts = new Map<string, { window: number; n: number }>();
  constructor(private readonly perMinute: number) {}

  /** 1回分を数え、上限を超えていなければ true。 */
  take(key: string, now: number = Date.now()): boolean {
    const window = Math.floor(now / 60_000);
    const c = this.counts.get(key);
    if (!c || c.window !== window) {
      // 古い窓の記録は、ここで捨てる（メモリを使い続けないように）
      if (this.counts.size > 10_000) for (const [k, v] of this.counts) if (v.window !== window) this.counts.delete(k);
      this.counts.set(key, { window, n: 1 });
      return this.perMinute > 0;
    }
    c.n += 1;
    return c.n <= this.perMinute;
  }
}

const results = new Map<string, { value: YomiSearchResult; at: number }>();

/** 覚えておいた結果があればそれを、なければ load の結果を覚えて返す。 */
export async function cachedSearch(params: SearchParams, load: () => Promise<YomiSearchResult>, now: number = Date.now()): Promise<YomiSearchResult> {
  const key = JSON.stringify([params.q, params.locale, params.mode, params.first]);
  const hit = results.get(key);
  if (hit && now - hit.at < RESULT_TTL_MS) return hit.value;
  const value = await load();
  if (results.size >= MAX_CACHED) {
    const oldest = results.keys().next().value;
    if (oldest !== undefined) results.delete(oldest);
  }
  results.delete(key);
  results.set(key, { value, at: now });
  return value;
}

/** 覚えておいた結果をすべて消す（CMSの記事の状態が変わったとき）。 */
export function clearSearchCache(): void {
  results.clear();
}
