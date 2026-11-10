// サイトのサーバーのミドルウェア: CMSのリダイレクト一覧に従って、旧URLを 301 で転送し、削除した記事のURLに 410 を返す
// （05-urls-and-redirects.md、Q40, Q145）。一覧はキャッシュし、CMSのWebhookで消す（webhook.ts）。
// CMSに届かないときは、古い一覧（なければ転送なし）で続け、サイトの表示は止めない。

import { createError, defineEventHandler, getRequestURL, sendRedirect } from "h3";
import { useRuntimeConfig } from "nitropack/runtime";
import queries from "#yomi/queries";
import protection from "#yomi/protected-paths";

import { cached } from "./cache.ts";
import { fetchQuery } from "./fetch-query.ts";
import { type YomiRedirect, type PathRule, isProtected, normalizePath, toPathRules, withoutProtected } from "./redirect-rules.ts";
import { REDIRECTS_QUERY } from "./names.ts";

export default defineEventHandler(async (event) => {
  if (event.method !== "GET" && event.method !== "HEAD") return;
  const config = useRuntimeConfig(event) as unknown as {
    yomi: { url: string; workspaceId: string; readKey: string; siteUrl: string };
    public: { yomi: { routePrefix: string } };
  };
  const url = getRequestURL(event);
  if (url.pathname.startsWith("/_nuxt/") || url.pathname.startsWith("/_yomi/")) return;
  // サイト自身のページとモジュールのルートには、CMS のリダイレクトを当てない（R26）
  if (isProtected(url.pathname, protection)) return;
  const id = Object.hasOwn(queries, REDIRECTS_QUERY) ? queries[REDIRECTS_QUERY] : undefined;
  if (!id || !config.yomi.readKey || !config.yomi.siteUrl) return;
  let rules: Map<string, PathRule>;
  try {
    rules = await cached("redirects", async () => {
      const data = (await fetchQuery(config.yomi, id, undefined)) as { redirects?: YomiRedirect[] } | null;
      return withoutProtected(toPathRules(data?.redirects ?? [], config.yomi.siteUrl), protection);
    });
  } catch {
    return;
  }
  const rule = rules.get(normalizePath(url.pathname + url.search)) ?? rules.get(normalizePath(url.pathname));
  if (!rule) return;
  if (rule.status === 410 || !rule.to) throw createError({ statusCode: 410, statusMessage: "Gone" });
  return sendRedirect(event, rule.to, 301);
});
