// 開いたURLの `#アンカー` が、訂正などで消えたセクションのものなら知らせる部品（06-corrections-and-freshness.md「削除されたセクション」、Q47）。
//   <YomiDeletedAnchorNotice :anchors="post.deletedAnchors" />
// - `#` 以降はサーバーに届かないので、ブラウザで照らし合わせる。`#` はルーターの現在のルートから取る（Nuxt は表示の直後に
//   URLを一時的に `#` なしに置き換えてから戻すため、そのときの location.hash は空になる。戻すときに hashchange は起きない）。削除済みアンカーの一覧は、記事のクエリでサイトのサーバーが取ったもの
//   （ブラウザから配信APIを呼ばない。原則1）
// - サーバーでの描画では何も出さない（`#` が分からないため。表示の食い違いも起きない）
// - 知らせの文は、CMSの説明文（訂正の説明。未記入ならワークスペースの定型文）。説明文がないとき（通常の更新で消えたときなど）は、
//   サイトが既定のスロットで文を渡す。スロットもなければ何も出さない。見た目はサイトが決める（クラスもスタイルも付けない）
// - 消えたセクションの本文は、配信APIも返さない（INV-COR-02）

import { type PropType, type SlotsType, defineComponent, getCurrentInstance, h, onBeforeUnmount, onMounted, ref, watch } from "vue";

/** 配信APIの `deletedAnchors` の1件。 */
export interface YomiDeletedAnchor {
  anchor: string;
  deletedAt: string;
  description: string | null;
}

/** URLの `#...` に当たる削除済みアンカーを探す。今の本文に同じIDがあれば（`exists`）、消えていないので知らせない。 */
export function findDeletedAnchor(
  hash: string,
  anchors: readonly YomiDeletedAnchor[],
  exists: (id: string) => boolean = () => false,
): YomiDeletedAnchor | undefined {
  if (!hash.startsWith("#") || hash.length < 2) return undefined;
  let id = hash.slice(1);
  try {
    id = decodeURIComponent(id);
  } catch {
    // 符号化が壊れていれば、そのままの文字で照らし合わせる
  }
  const found = anchors.find((a) => a.anchor === id);
  return found && !exists(id) ? found : undefined;
}

export default defineComponent({
  name: "YomiDeletedAnchorNotice",
  props: {
    /** 配信APIの記事の `deletedAnchors`。 */
    anchors: { type: Array as PropType<YomiDeletedAnchor[] | null | undefined>, default: () => [] },
  },
  slots: Object as SlotsType<{ default: { anchor: YomiDeletedAnchor } }>,
  setup(props, { slots }) {
    const current = ref<YomiDeletedAnchor>();
    // vue-router の現在のルート（Nuxt では常にある。なければ location.hash だけを見る）
    const instance = getCurrentInstance();
    const routeHash = () => (instance?.proxy as { $route?: { hash?: unknown } } | null)?.$route?.hash;
    const check = () => {
      const hash = routeHash();
      current.value = findDeletedAnchor(typeof hash === "string" && hash ? hash : window.location.hash, props.anchors ?? [], (id) => document.getElementById(id) !== null);
    };
    onMounted(() => {
      check();
      window.addEventListener("hashchange", check);
      // ルートの `#` が変わったときと、同じページの部品のまま別の記事に移ったとき
      watch([routeHash, () => props.anchors], check);
    });
    onBeforeUnmount(() => window.removeEventListener("hashchange", check));
    return () => {
      const anchor = current.value;
      if (!anchor) return null;
      const content = slots.default ? slots.default({ anchor }) : anchor.description;
      if (!content) return null;
      return h("aside", { role: "status", "data-yomi-deleted-anchor": anchor.anchor }, content);
    };
  },
});
