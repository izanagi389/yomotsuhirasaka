// サイトのサーバーの、サイト内検索のルートの中身（Nitro に依存しない。テストで直接呼ぶ）。
// - 訪問者の入力のたびに呼ばれ、ベクトル検索ならワークスペースの埋め込みの予算も減るので、訪問者ごとに回数を制限する。
//   CMSの配信APIの回数制限はAPIキーごとで、サイトの訪問者全員で共有になるため
// - 数えるのはサイトのサーバーのメモリの中だけ（複数のプロセスで動かすと、プロセスごとに数える）
// - 同じ検索（検索語・ロケール・モード・件数）の結果を、短い時間だけ覚えておく。CMSのWebhookを受けたら消す

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

/** ルートのクエリ文字列（`q`・`locale`・`mode`・`first`）を確かめる。 */
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
      if (value) return value;
    }
  }
  return socketIp ?? "unknown";
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
