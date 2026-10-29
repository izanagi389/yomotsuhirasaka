// サイトのサーバーのルート `<search.route>?q=&locale=&mode=&first=`（サイト内検索）。
// ブラウザから配信APIを直接呼ばせない（原則1）。モジュールが登録した検索のクエリだけを呼ぶ。
// 訪問者ごとの回数の制限・検索語の長さ・短い時間の結果のキャッシュは search-core.ts。

import { createError, defineEventHandler, getQuery, getRequestHeaders, getRequestIP, setResponseHeader } from "h3";
import { useRuntimeConfig } from "nitropack/runtime";
// ビルドのときに作る、クエリの名前 → 登録したID の対応（モジュールが Nitro の仮想モジュールとして渡す）
import queries from "#cms/queries";

import type { CmsSearchResult } from "../search.ts";
import { QueryError, fetchQuery } from "./fetch-query.ts";
import { SEARCH_QUERY } from "./names.ts";
import { EMPTY_RESULT, RateLimiter, cachedSearch, clientIp, parseSearchParams } from "./search-core.ts";

let limiter: RateLimiter | null = null;

export default defineEventHandler(async (event) => {
  const config = useRuntimeConfig(event) as unknown as {
    cms: { url: string; workspaceId: string; readKey: string; search: { perMinute: number; ipHeader: string } };
  };
  const id = queries[SEARCH_QUERY];
  if (!id) throw createError({ statusCode: 404, statusMessage: "Unknown query" });
  if (!config.cms.readKey) throw createError({ statusCode: 500, statusMessage: "The CMS read key is not configured" });
  // 検索の結果は訪問者ごとに違いうるので、共有のキャッシュ（CDN）には置かせない
  setResponseHeader(event, "Cache-Control", "private, no-store");
  try {
    const params = parseSearchParams(getQuery(event));
    if (!params.q) return EMPTY_RESULT;
    limiter ??= new RateLimiter(config.cms.search.perMinute);
    const ip = clientIp(getRequestHeaders(event), getRequestIP(event), config.cms.search.ipHeader);
    if (!limiter.take(ip)) throw createError({ statusCode: 429, statusMessage: "Too Many Requests" });
    return await cachedSearch(params, async () => {
      const data = (await fetchQuery(config.cms, id, { text: params.q, locale: params.locale, mode: params.mode, first: params.first })) as {
        search?: CmsSearchResult;
      } | null;
      if (!data?.search) throw new QueryError(502, "The CMS returned no search result");
      return data.search;
    });
  } catch (e) {
    if (e instanceof QueryError) throw createError({ statusCode: e.status, statusMessage: e.message });
    throw e;
  }
});
