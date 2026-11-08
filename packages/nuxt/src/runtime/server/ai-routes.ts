// サイトのサーバーの、AI向けの素材のルート（07-ai-delivery.md、Q145）。
// - ミドルウェア: `<記事のURLのパス>.md` に、記事のMarkdown版を返す。目録にある記事だけをCMSに問い合わせる
//   （でたらめなパスでCMSへの問い合わせやキャッシュが増えないように）。目録にないパスは、サイトの処理に任せる
// - `/llms.txt` と `/robots.txt`（設定したとき）
// CMSに届かないときは古いもの（なければ 502）を返す。

import { createError, defineEventHandler, getRequestURL, setResponseHeader } from "h3";
import type { H3Event } from "h3";
import { useRuntimeConfig } from "nitropack/runtime";
import queries from "#yomi/queries";

import { type AiConfig, articlePath, toLlmsTxt, toRobotsTxt } from "./ai.ts";
import { loadCatalog, loadMarkdown, loadOptOuts } from "./ai-load.ts";
import { QueryError, type QueryTarget } from "./fetch-query.ts";
import { AI_CATALOG_QUERY, AI_MARKDOWN_QUERY, AI_OPT_OUTS_QUERY } from "./names.ts";

function setup(event: H3Event): { target: QueryTarget; ai: AiConfig } | null {
  const config = useRuntimeConfig(event) as unknown as { yomi: QueryTarget & { ai: AiConfig | null } };
  if (!config.yomi.ai || !config.yomi.readKey) return null;
  return { target: config.yomi, ai: config.yomi.ai };
}

const queryId = (name: string): string | undefined => (Object.hasOwn(queries, name) ? queries[name] : undefined);

async function guarded<T>(load: () => Promise<T>): Promise<T> {
  try {
    return await load();
  } catch (e) {
    if (e instanceof QueryError) throw createError({ statusCode: e.status, statusMessage: e.message });
    throw e;
  }
}

/** `<記事のURLのパス>.md`。 */
export const markdownMiddleware = defineEventHandler(async (event) => {
  if (event.method !== "GET" && event.method !== "HEAD") return;
  const path = getRequestURL(event).pathname;
  if (!path.endsWith(".md") || path.startsWith("/_nuxt/") || path.startsWith("/_yomi/")) return;
  const s = setup(event);
  const catalogId = queryId(AI_CATALOG_QUERY);
  const markdownId = queryId(AI_MARKDOWN_QUERY);
  if (!s || !s.ai.markdown || !catalogId || !markdownId) return;
  const base = path.slice(0, -".md".length);
  let url: string | undefined;
  for (const { locale } of s.ai.locales) {
    const items = await guarded(() => loadCatalog(s.target, catalogId, locale));
    url = items.find((i) => articlePath(i.url, s.ai.siteOrigin) === base)?.url;
    if (url) break;
  }
  if (!url) return;
  const md = await guarded(() => loadMarkdown(s.target, markdownId, url));
  if (md === null) return;
  setResponseHeader(event, "Content-Type", "text/markdown; charset=utf-8");
  return md;
});

/** `/llms.txt`。 */
export const llmsTxtHandler = defineEventHandler(async (event) => {
  const s = setup(event);
  const catalogId = queryId(AI_CATALOG_QUERY);
  if (!s || !s.ai.llmsTxt || !catalogId) throw createError({ statusCode: 404, statusMessage: "Not Found" });
  const catalogs: Record<string, Awaited<ReturnType<typeof loadCatalog>>> = {};
  for (const { locale } of s.ai.locales) catalogs[locale] = await guarded(() => loadCatalog(s.target, catalogId, locale));
  setResponseHeader(event, "Content-Type", "text/plain; charset=utf-8");
  return toLlmsTxt(s.ai, catalogs);
});

/** `/robots.txt`。 */
export const robotsTxtHandler = defineEventHandler(async (event) => {
  const s = setup(event);
  const optOutsId = queryId(AI_OPT_OUTS_QUERY);
  if (!s || !s.ai.robotsTxt || !optOutsId) throw createError({ statusCode: 404, statusMessage: "Not Found" });
  const urls = await guarded(() => loadOptOuts(s.target, optOutsId));
  setResponseHeader(event, "Content-Type", "text/plain; charset=utf-8");
  return toRobotsTxt(s.ai, urls);
});
