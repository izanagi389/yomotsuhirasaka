// 画面から登録済みのクエリを使う（`const { data } = await useCmsQuery<{ postBySlug: Post | null }>("postBySlug", { slug })`）。
// サイトのサーバーのルートを通して取る。SSG では、ビルドのときの結果（payload）を使う。

import { useFetch, useRuntimeConfig } from "nuxt/app";
import { type MaybeRefOrGetter, computed, toValue } from "vue";

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
