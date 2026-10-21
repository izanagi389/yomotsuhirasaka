// サイトのサーバーのミドルウェア: CMSのリダイレクト一覧に従って、旧URLを 301 で転送し、削除した記事のURLに 410 を返す
// （05-urls-and-redirects.md、Q40, Q145）。一覧はキャッシュし、CMSのWebhookで消す（webhook.ts）。
// CMSに届かないときは、古い一覧（なければ転送なし）で続け、サイトの表示は止めない。

import { createError, defineEventHandler, getRequestURL, sendRedirect } from "h3";
import { useRuntimeConfig } from "nitropack/runtime";
import queries from "#cms/queries";

import { cached } from "./cache.ts";
import { fetchQuery } from "./fetch-query.ts";
import { type CmsRedirect, type PathRule, normalizePath, toPathRules } from "./redirect-rules.ts";
import { REDIRECTS_QUERY } from "./names.ts";

export default defineEventHandler(async (event) => {
  if (event.method !== "GET" && event.method !== "HEAD") return;
  const config = useRuntimeConfig(event) as unknown as {
    cms: { url: string; workspaceId: string; readKey: string; siteUrl: string };
    public: { cms: { routePrefix: string } };
  };
  const url = getRequestURL(event);
  if (url.pathname.startsWith("/_nuxt/") || url.pathname.startsWith("/_cms/")) return;
  const id = Object.hasOwn(queries, REDIRECTS_QUERY) ? queries[REDIRECTS_QUERY] : undefined;
  if (!id || !config.cms.readKey || !config.cms.siteUrl) return;
  let rules: Map<string, PathRule>;
  try {
    rules = await cached("redirects", async () => {
      const data = (await fetchQuery(config.cms, id, undefined)) as { redirects?: CmsRedirect[] } | null;
      return toPathRules(data?.redirects ?? [], config.cms.siteUrl);
    });
  } catch {
    return;
  }
  const rule = rules.get(normalizePath(url.pathname + url.search)) ?? rules.get(normalizePath(url.pathname));
  if (!rule) return;
  if (rule.status === 410 || !rule.to) throw createError({ statusCode: 410, statusMessage: "Gone" });
  return sendRedirect(event, rule.to, 301);
});
