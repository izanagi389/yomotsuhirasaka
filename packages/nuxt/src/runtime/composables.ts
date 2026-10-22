// 画面から登録済みのクエリを使う（`const { data } = await useCmsQuery<{ postBySlug: Post | null }>("postBySlug", { slug })`）。
// サイトのサーバーのルートを通して取る。SSG では、ビルドのときの結果（payload）を使う。

import { useFetch, useHead, useRuntimeConfig } from "nuxt/app";
import { type MaybeRefOrGetter, computed, toValue } from "vue";

import type { CmsCorrectionItem } from "./server/feed.ts";

export function useCmsQuery<T = unknown>(name: string, variables?: MaybeRefOrGetter<Record<string, unknown> | undefined>) {
  const prefix = (useRuntimeConfig().public as { cms?: { routePrefix?: string } }).cms?.routePrefix ?? "/_cms/q";
  const query = computed(() => {
    const v = toValue(variables);
    return v ? { v: JSON.stringify(v) } : {};
  });
  return useFetch<T>(`${prefix}/${encodeURIComponent(name)}`, {
    query,
    key: () => `cms:${name}:${query.value.v ?? ""}`,
  });
}

/**
 * 訂正の一覧（新しい順）。nuxt.config で `cms.corrections` を設定したときに使える。
 * `const { data } = await useCmsCorrections({ locale: "ja", limit: 20, offset: 0 })` → `data.value?.corrections`
 */
export function useCmsCorrections(variables: MaybeRefOrGetter<{ locale: string; limit?: number; offset?: number }>) {
  return useCmsQuery<{ corrections: CmsCorrectionItem[] }>("cmsCorrections", variables);
}

/**
 * 学習への利用を拒否した記事のページに、`<meta name="robots" content="noai, noimageai">` を足す（07-ai-delivery.md）。
 * 記事のクエリで `aiTrainingOptOut` を取って渡す（`useCmsAiMeta(() => data.value?.postBySlug)`）。
 * クローラーへの意思表示であって、技術的に止めるものではない。
 */
export function useCmsAiMeta(entry: MaybeRefOrGetter<{ aiTrainingOptOut?: boolean | null } | null | undefined>) {
  useHead(() => ({
    meta: toValue(entry)?.aiTrainingOptOut === true ? [{ name: "robots", content: "noai, noimageai", key: "cms-ai-training" }] : [],
  }));
}
