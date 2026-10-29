// サイト内検索の、飾りのない意味のあるHTMLの部品（`<CmsSearchBox label="サイト内を検索" locale="ja" />`）。
// - 見た目はサイトが決める（原則1）。クラスもスタイルも付けない
// - 部品の中に決まった文言を持たない。ラベルは必ず渡す。読み込み中・結果なし・予算切れ・キーワード検索が使えない・
//   回数の制限・失敗は、スロット（loading・empty・budgetExhausted・keywordDisabled・rateLimited・error）でだけ描く
// - 結果1件の描き方は `hit` のスロットで置き換えられる。既定は、パンくず（なければURL）のリンクと、一致した箇所を
//   `<mark>` で囲んだ抜粋。`v-html` は使わない
// - 検索は useCmsSearch（サイトのサーバーのルートを通す。ブラウザから配信APIを直接呼ばない）

import { type PropType, type SlotsType, type VNode, defineComponent, h, useId } from "vue";

import { useCmsSearch } from "./composables.ts";
import { hrefAllowed } from "./href.ts";
import { type CmsHighlightSegment, type CmsSearchHit, type CmsSearchMode, MAX_QUERY_CHARS, cmsHighlight } from "./search.ts";

/** 結果のリンク先（記事のURLと、当たったセクションのアンカー）。描いてよいURLでなければ null。 */
export function hitHref(hit: CmsSearchHit): string | null {
  if (!hit.url) return null;
  const href = hit.anchor ? `${hit.url}#${encodeURIComponent(hit.anchor)}` : hit.url;
  return hrefAllowed(href) ? href : null;
}

export default defineComponent({
  name: "CmsSearchBox",
  props: {
    /** 検索欄のラベル（必須。サイトのロケールの文言で）。 */
    label: { type: String, required: true },
    /** 検索するロケール（必須）。 */
    locale: { type: String, required: true },
    mode: { type: String as PropType<CmsSearchMode>, default: undefined },
    first: { type: Number, default: undefined },
    placeholder: { type: String, default: undefined },
    debounceMs: { type: Number, default: undefined },
  },
  slots: Object as SlotsType<{
    hit: { hit: CmsSearchHit; href: string | null; segments: CmsHighlightSegment[] };
    loading: Record<string, never>;
    empty: Record<string, never>;
    budgetExhausted: Record<string, never>;
    keywordDisabled: Record<string, never>;
    rateLimited: Record<string, never>;
    error: Record<string, never>;
  }>,
  setup(props, { slots }) {
    const { text, result, status } = useCmsSearch({
      locale: () => props.locale,
      mode: () => props.mode,
      first: () => props.first,
      ...(props.debounceMs === undefined ? {} : { debounceMs: props.debounceMs }),
    });
    const inputId = useId();
    const renderHit = (hit: CmsSearchHit): VNode => {
      const href = hitHref(hit);
      const segments = cmsHighlight(hit.excerpt, hit.matches);
      const content = slots.hit
        ? slots.hit({ hit, href, segments })
        : [
            href ? h("a", { href }, hit.breadcrumb.length > 0 ? hit.breadcrumb.join(" › ") : hit.url ?? "") : null,
            h(
              "p",
              segments.map((s) => (s.match ? h("mark", s.text) : s.text)),
            ),
          ];
      return h("li", { key: `${hit.entryId}#${hit.anchor ?? ""}` }, content);
    };
    return () => {
      const r = result.value;
      const notices: (VNode[] | undefined)[] = [];
      if (status.value === "loading") notices.push(slots.loading?.({}));
      if (status.value === "rateLimited") notices.push(slots.rateLimited?.({}));
      if (status.value === "error") notices.push(slots.error?.({}));
      if (status.value === "done" && r) {
        if (r.vectorSearch === "BUDGET_EXHAUSTED") notices.push(slots.budgetExhausted?.({}));
        if (r.keywordSearch === "DISABLED") notices.push(slots.keywordDisabled?.({}));
        if (r.hits.length === 0) notices.push(slots.empty?.({}));
      }
      return h("search", [
        h("label", { for: inputId }, props.label),
        h("input", {
          id: inputId,
          type: "search",
          value: text.value,
          maxlength: MAX_QUERY_CHARS,
          placeholder: props.placeholder,
          autocomplete: "off",
          onInput: (e: Event) => {
            text.value = (e.target as HTMLInputElement).value;
          },
        }),
        h("div", { role: "status", "aria-live": "polite" }, notices.flat().filter(Boolean) as VNode[]),
        r && status.value === "done" && r.hits.length > 0 ? h("ol", r.hits.map(renderHit)) : null,
      ]);
    };
  },
});
