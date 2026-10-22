// AI向けの素材を配信APIから取る（サイトのサーバーの中だけ）。取ったものはキャッシュし、CMSのWebhookで消す（cache.ts）。
// ここは Nitro に依存しない（ビルドのときにも呼ぶ）。

import { cached } from "./cache.ts";
import type { CmsAiCatalogItem } from "./ai.ts";
import { type QueryTarget, fetchQuery } from "./fetch-query.ts";

/** 配信APIが1回に返す目録の件数の上限。 */
const PAGE = 1000;
/** 1つのロケールで読む目録の上限（ページ数）。 */
const MAX_PAGES = 20;

/** ロケールの目録をすべて（上限まで）取る。 */
export function loadCatalog(target: QueryTarget, id: string, locale: string): Promise<CmsAiCatalogItem[]> {
  return cached(`ai:catalog:${locale}`, async () => {
    const out: CmsAiCatalogItem[] = [];
    for (let page = 0; page < MAX_PAGES; page++) {
      const data = (await fetchQuery(target, id, { locale, limit: PAGE, offset: page * PAGE })) as { aiCatalog?: CmsAiCatalogItem[] } | null;
      const items = data?.aiCatalog ?? [];
      out.push(...items);
      if (items.length < PAGE) break;
    }
    return out;
  });
}

/** 公開URLで記事のMarkdown版を取る（素材にならない記事では null）。 */
export function loadMarkdown(target: QueryTarget, id: string, url: string): Promise<string | null> {
  return cached(`ai:markdown:${url}`, async () => {
    const data = (await fetchQuery(target, id, { url })) as { aiMarkdownByUrl?: string | null } | null;
    return data?.aiMarkdownByUrl ?? null;
  });
}

/** 学習への利用を拒否した記事のURL。 */
export function loadOptOuts(target: QueryTarget, id: string): Promise<string[]> {
  return cached("ai:opt-outs", async () => {
    const data = (await fetchQuery(target, id, undefined)) as { aiTrainingOptOuts?: string[] } | null;
    return data?.aiTrainingOptOuts ?? [];
  });
}
