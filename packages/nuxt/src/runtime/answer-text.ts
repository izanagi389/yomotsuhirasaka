// チャットボットの回答の本文の、飾りのない意味のあるHTMLの部品（`<CmsAnswerText :text="state.text" :sources="state.sources" />`）。
// - 回答は Markdown。描くのは段落・箇条書き・太字・リンク・インラインのコード・コードのかたまりだけで、ほかの書き方は文字のまま。
//   画像は描かない。`v-html` を使わず、文字はすべて文字として描く（13-security.md「公式SDKは、表示する前にエスケープする」）
// - 本文の `[n]` は、`citation` が同じ出典のセクションへのリンク（`<sup><a href="url#anchor">[n]</a></sup>`）。
//   描き方は `citation` のスロットで置き換えられる。出典がなければ文字のまま
// - クラスもスタイルも付けない。見た目と出典の一覧はサイトが決める（原則1）

import { type PropType, type SlotsType, type VNode, defineComponent, h } from "vue";

import { type CmsAnswerInline, type CmsAnswerSource, parseAnswerMarkdown, sourceHref } from "./answer.ts";

export default defineComponent({
  name: "CmsAnswerText",
  props: {
    /** 回答の本文（Markdown）。 */
    text: { type: String, required: true },
    /** 出典（`[n]` のリンク先）。 */
    sources: { type: Array as PropType<CmsAnswerSource[]>, default: () => [] },
  },
  slots: Object as SlotsType<{
    citation: { n: number; source: CmsAnswerSource | undefined; href: string | null };
  }>,
  setup(props, { slots }) {
    const inline = (nodes: CmsAnswerInline[]): (VNode | string)[] =>
      nodes.map((n): VNode | string => {
        switch (n.type) {
          case "text":
            return n.text;
          case "strong":
            return h("strong", inline(n.children));
          case "code":
            return h("code", n.text);
          case "link":
            return h("a", { href: n.href, rel: "noopener noreferrer nofollow" }, inline(n.children));
          case "citation": {
            const source = props.sources.find((s) => s.citation === n.n);
            const href = sourceHref(source);
            if (slots.citation) return h("span", slots.citation({ n: n.n, source, href }));
            return href ? h("sup", [h("a", { href }, `[${n.n}]`)]) : `[${n.n}]`;
          }
        }
      });
    return () =>
      h(
        "div",
        parseAnswerMarkdown(props.text).map((b) => {
          switch (b.type) {
            case "paragraph":
              return h("p", inline(b.children));
            case "list":
              return h(b.ordered ? "ol" : "ul", b.items.map((item) => h("li", inline(item))));
            case "code":
              return h("pre", [h("code", b.text)]);
          }
        }),
      );
  },
});
