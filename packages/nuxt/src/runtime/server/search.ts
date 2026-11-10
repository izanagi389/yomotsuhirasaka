// サイトのサーバーのルート `POST <search.route>`（サイト内検索。本文は JSON の `{ q, locale, mode?, first? }`）。
// 検索語は URL に載せない（プロキシやアクセスログに残さない）。CMS への問い合わせも POST で送る。
// ブラウザから配信APIを直接呼ばせない（原則1）。モジュールが登録した検索のクエリだけを呼ぶ。
// 訪問者ごとの回数の制限・検索語の長さ・短い時間の結果のキャッシュは search-core.ts。

import { createError, defineEventHandler, getRequestHeaders, getRequestIP, getRequestWebStream, setResponseHeader } from "h3";
import { useRuntimeConfig } from "nitropack/runtime";
// ビルドのときに作る、クエリの名前 → 登録したID の対応（モジュールが Nitro の仮想モジュールとして渡す）
import queries from "#yomi/queries";

import type { YomiSearchResult } from "../search.ts";
import { QueryError, fetchQuery } from "./fetch-query.ts";
import { SEARCH_QUERY } from "./names.ts";
import { EMPTY_RESULT, MAX_SEARCH_BODY_BYTES, RateLimiter, cachedSearch, clientIp, parseSearchBody } from "./search-core.ts";
import { readLimitedText } from "./read-body.ts";

let limiter: RateLimiter | null = null;

export default defineEventHandler(async (event) => {
  const config = useRuntimeConfig(event) as unknown as {
    yomi: { url: string; workspaceId: string; readKey: string; search: { perMinute: number; ipHeader: string } };
  };
  const id = queries[SEARCH_QUERY];
  if (!id) throw createError({ statusCode: 404, statusMessage: "Unknown query" });
  if (!config.yomi.readKey) throw createError({ statusCode: 500, statusMessage: "The CMS read key is not configured" });
  // 検索の結果は訪問者ごとに違いうるので、共有のキャッシュ（CDN）には置かせない
  setResponseHeader(event, "Cache-Control", "private, no-store");
  try {
    const params = parseSearchBody(await readLimitedText(getRequestWebStream(event), MAX_SEARCH_BODY_BYTES));
    if (!params.q) return EMPTY_RESULT;
    limiter ??= new RateLimiter(config.yomi.search.perMinute);
    const ip = clientIp(getRequestHeaders(event), getRequestIP(event), config.yomi.search.ipHeader);
    if (!limiter.take(ip)) throw createError({ statusCode: 429, statusMessage: "Too Many Requests" });
    return await cachedSearch(params, async () => {
      const variables = { text: params.q, locale: params.locale, mode: params.mode, first: params.first };
      const data = (await fetchQuery(config.yomi, id, variables, fetch, "POST")) as {
        search?: YomiSearchResult;
      } | null;
      if (!data?.search) throw new QueryError(502, "The CMS returned no search result");
      return data.search;
    });
  } catch (e) {
    if (e instanceof QueryError) throw createError({ statusCode: e.status, statusMessage: e.message });
    throw e;
  }
});
