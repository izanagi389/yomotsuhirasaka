// 訂正の一覧を、飾りのない意味のあるHTMLとして描く部品（`<YomiCorrections :items="data.corrections" />`）。
// - 見た目はサイトが決める（原則1）。クラスもスタイルも付けない。1件ごとの描き方は `item` のスロットで置き換えられる
// - 載せるのは配信APIが返すもの（説明文・影響したセクション）だけ。変更前の本文は配信APIも返さない（INV-COR-02）
// - 日付は、サーバーとブラウザで同じになるよう、UTC の YYYY-MM-DD で書く。ほかの書き方はスロットで

import { type PropType, type SlotsType, defineComponent, h } from "vue";

import { hrefAllowed } from "./href.ts";
import type { YomiCorrectionItem } from "./server/feed.ts";

export default defineComponent({
  name: "YomiCorrections",
  props: {
    /** 配信APIの `corrections`（useYomiCorrections の `data.corrections`）。 */
    items: { type: Array as PropType<YomiCorrectionItem[] | null | undefined>, default: () => [] },
  },
  slots: Object as SlotsType<{ item: { item: YomiCorrectionItem } }>,
  setup(props, { slots }) {
    return () => {
      const items = props.items ?? [];
      if (items.length === 0) return null;
      return h(
        "ol",
        items.map((item) =>
          h("li", { key: `${item.entryId}@${item.correctedAt}` }, slots.item ? slots.item({ item }) : [
            h("time", { datetime: item.correctedAt }, item.correctedAt.slice(0, 10)),
            " ",
            hrefAllowed(item.url) ? h("a", { href: item.url }, item.title ?? item.url) : (item.title ?? ""),
            item.description ? h("p", item.description) : null,
          ]),
        ),
      );
    };
  },
});
