// 画面から登録済みのクエリを使う（`const { data } = await useCmsQuery<{ postBySlug: Post | null }>("postBySlug", { slug })`）。
// サイトのサーバーのルートを通して取る。SSG では、ビルドのときの結果（payload）を使う。

import { useFetch, useHead, useRuntimeConfig } from "nuxt/app";
import { type MaybeRefOrGetter, type Ref, computed, onScopeDispose, ref, toValue, watch } from "vue";

import { type CmsAnswerErrorCode, type CmsAnswerEvent, type CmsAnswerState, INITIAL_ANSWER_STATE, SseReader, applyAnswerEvent } from "./answer.ts";
import { type CmsSearchMode, type CmsSearchResult, MAX_QUERY_CHARS } from "./search.ts";
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

export interface CmsSearchOptions {
  /** 検索するロケール（必須。サイトのロケールの決め方はサイトごとに違うので、モジュールは推測しない）。 */
  locale: MaybeRefOrGetter<string>;
  /** 既定は HYBRID。 */
  mode?: MaybeRefOrGetter<CmsSearchMode | undefined>;
  /** 件数（1〜50、既定は 10）。 */
  first?: MaybeRefOrGetter<number | undefined>;
  /** 入力が止まってから検索するまでの時間（ミリ秒、既定は 300）。 */
  debounceMs?: number;
}

/** 検索の状態。rateLimited は、サイトのサーバーの訪問者ごとの回数の制限を超えたとき（429）。 */
export type CmsSearchStatus = "idle" | "loading" | "done" | "error" | "rateLimited";

/**
 * サイト内検索（nuxt.config で `cms.search` を設定したときに使える）。`text` に入力を入れると、入力が止まってから
 * サイトのサーバーのルートを通して検索する。後から始めた検索の結果が先に届いても、古い結果では上書きしない。
 * 検索はブラウザでだけ行う（サーバーでの描画のときは idle のまま）。
 * `const { text, result, status } = useCmsSearch({ locale: "ja" })`
 */
export function useCmsSearch(options: CmsSearchOptions): {
  text: Ref<string>;
  result: Ref<CmsSearchResult | null>;
  status: Ref<CmsSearchStatus>;
  error: Ref<unknown>;
} {
  const route = (useRuntimeConfig().public as { cms?: { searchRoute?: string } }).cms?.searchRoute ?? "";
  const text = ref("");
  const result = ref<CmsSearchResult | null>(null);
  const status = ref<CmsSearchStatus>("idle");
  const error = ref<unknown>(null);
  let timer: ReturnType<typeof setTimeout> | undefined;
  let seq = 0;

  async function run(n: number, q: string, locale: string, mode: CmsSearchMode | undefined, first: number | undefined) {
    try {
      const query = new URLSearchParams({ q, locale });
      if (mode) query.set("mode", mode);
      if (first !== undefined) query.set("first", String(first));
      const res = await fetch(`${route}?${query}`, { headers: { Accept: "application/json" } });
      if (n !== seq) return;
      if (res.status === 429) {
        status.value = "rateLimited";
        return;
      }
      if (!res.ok) throw new Error(`search failed: ${res.status}`);
      const r = (await res.json()) as CmsSearchResult;
      if (n !== seq) return;
      result.value = r;
      status.value = "done";
    } catch (e) {
      if (n !== seq) return;
      error.value = e;
      status.value = "error";
    }
  }

  if (typeof window !== "undefined") {
    if (!route) throw new Error("useCmsSearch: set cms.search in nuxt.config");
    watch(
      () => [text.value, toValue(options.locale), toValue(options.mode), toValue(options.first)] as const,
      ([raw, locale, mode, first]) => {
        clearTimeout(timer);
        const n = ++seq;
        const q = raw.trim().slice(0, MAX_QUERY_CHARS);
        error.value = null;
        if (!q) {
          result.value = null;
          status.value = "idle";
          return;
        }
        status.value = "loading";
        timer = setTimeout(() => void run(n, q, locale, mode, first), options.debounceMs ?? 300);
      },
    );
    onScopeDispose(() => clearTimeout(timer));
  }
  return { text, result, status, error };
}

/**
 * チャットボットの回答（nuxt.config で `cms.answer` を設定したときに使える）。`ask(question, locale)` で送り、
 * 判定・本文の断片・最終判定を、届いた順に state に反映する。送り直すと前の回答は止める。`stop()` で止める。
 * 送るのはサイトのサーバーのルート（ブラウザから CMS を直接呼ばない）。本文は <CmsAnswerText> で描く（v-html は使わない）。
 */
export function useCmsAnswer(): {
  state: Ref<CmsAnswerState>;
  ask: (question: string, locale: string) => Promise<void>;
  stop: () => void;
} {
  const route = (useRuntimeConfig().public as { cms?: { answerRoute?: string } }).cms?.answerRoute ?? "";
  const state = ref<CmsAnswerState>({ ...INITIAL_ANSWER_STATE });
  let controller: AbortController | null = null;

  function stop() {
    controller?.abort();
    controller = null;
  }

  async function ask(question: string, locale: string) {
    if (!route) throw new Error("useCmsAnswer: set cms.answer in nuxt.config");
    stop();
    const mine = new AbortController();
    controller = mine;
    let s: CmsAnswerState = { ...INITIAL_ANSWER_STATE, phase: "asking" };
    state.value = s;
    const apply = (e: CmsAnswerEvent) => {
      if (controller !== mine) return;
      s = applyAnswerEvent(s, e);
      state.value = s;
    };
    try {
      const res = await fetch(route, {
        method: "POST",
        headers: { "Content-Type": "application/json", Accept: "text/event-stream" },
        body: JSON.stringify({ question, locale }),
        signal: mine.signal,
      });
      if (!res.ok || !res.body) {
        let code: CmsAnswerErrorCode = res.status === 429 ? "RATE_LIMITED" : "ANSWER_FAILED";
        try {
          const body = (await res.json()) as { code?: CmsAnswerErrorCode };
          if (body.code) code = body.code;
        } catch {
          // 本文が読めなければ、状態コードから決めたコードのまま
        }
        const after = Number(res.headers.get("retry-after"));
        apply({ type: "error", code, ...(Number.isFinite(after) && after > 0 ? { retryAfter: after } : {}) });
        return;
      }
      const reader = res.body.getReader();
      const sse = new SseReader();
      const decoder = new TextDecoder();
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        for (const { event, data } of sse.push(decoder.decode(value, { stream: true }))) {
          apply({ type: event, ...(JSON.parse(data) as object) } as CmsAnswerEvent);
        }
      }
      if (s.phase !== "done" && s.phase !== "error") apply({ type: "error", code: "ANSWER_FAILED" });
    } catch {
      // 止めたときは何もしない
      if (!mine.signal.aborted) apply({ type: "error", code: "ANSWER_FAILED" });
    } finally {
      if (controller === mine) controller = null;
    }
  }

  onScopeDispose(stop);
  return { state, ask, stop };
}
