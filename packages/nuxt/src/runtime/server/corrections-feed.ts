// サイトのサーバーのルート `<path>.xml`（RSS 2.0）と `<path>.json`（JSON Feed 1.1）: 訂正フィード（Q48）。
// 配信APIから取ったものはキャッシュし、CMSのWebhookで消す（cache.ts、Q145, Q146）。`nuxt generate` ではビルドのときに書き出す。
// CMSに届かないときは古いもの（なければ 502）を返す。

import { createError, defineEventHandler, getRequestURL, setResponseHeader } from "h3";
import { useRuntimeConfig } from "nitropack/runtime";
import queries from "#cms/queries";

import { cached } from "./cache.ts";
import { type CmsCorrectionItem, type CorrectionFeedConfig, toJsonFeed, toRss } from "./feed.ts";
import { QueryError, fetchQuery } from "./fetch-query.ts";
import { CORRECTIONS_QUERY } from "./names.ts";

export default defineEventHandler(async (event) => {
  const config = useRuntimeConfig(event) as unknown as {
    cms: { url: string; workspaceId: string; readKey: string; correctionFeeds: Record<string, CorrectionFeedConfig> };
  };
  const m = /^(.+)\.(xml|json)$/.exec(getRequestURL(event).pathname);
  const feed = m && Object.hasOwn(config.cms.correctionFeeds ?? {}, m[1]!) ? config.cms.correctionFeeds[m[1]!] : undefined;
  const id = Object.hasOwn(queries, CORRECTIONS_QUERY) ? queries[CORRECTIONS_QUERY] : undefined;
  if (!m || !feed || !id) throw createError({ statusCode: 404, statusMessage: "Not Found" });
  if (!config.cms.readKey) throw createError({ statusCode: 500, statusMessage: "The CMS read key is not configured" });
  let items: CmsCorrectionItem[];
  try {
    items = await cached(`corrections:${feed.locale}:${feed.limit}`, async () => {
      const data = (await fetchQuery(config.cms, id, { locale: feed.locale, limit: feed.limit, offset: 0 })) as { corrections?: CmsCorrectionItem[] } | null;
      return data?.corrections ?? [];
    });
  } catch (e) {
    if (e instanceof QueryError) throw createError({ statusCode: e.status, statusMessage: e.message });
    throw e;
  }
  if (m[2] === "xml") {
    setResponseHeader(event, "Content-Type", "application/rss+xml; charset=utf-8");
    return toRss(feed, items);
  }
  setResponseHeader(event, "Content-Type", "application/feed+json; charset=utf-8");
  return toJsonFeed(feed, items);
});
