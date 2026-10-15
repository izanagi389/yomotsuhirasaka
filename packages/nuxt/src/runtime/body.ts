// 本文（Tiptapの構造化JSON）を、飾りのない意味のあるHTMLとして描く部品（`<CmsBody :doc="post.body" />`）。
// - 見た目はサイトが決める（原則1）。クラスもスタイルも付けない。ノードやマークごとに、サイトの部品で置き換えられる
//   （`:components="{ image: MyImage, embed: MyEmbed }"`。部品は `node` を受け取り、子は既定のスロットに入る）
// - v-html を使わず、仮想DOMで組み立てる（文字はすべて文字として描く）
// - リンクと画像のURLは描く前にも確かめ、使えないものはリンクや画像にしない
// - 埋め込みは、既定では元のURLへのリンクにする（iframe を出すかどうかはサイトのCSPなどで決めるため、サイトの部品で置き換える）
// - 見出しには、CMSが振った固定のアンカーID（Q28）を id に付ける。共有されたリンクの `#` と一致する

import { type Component, type PropType, type VNode, type VNodeArrayChildren, defineComponent, h } from "vue";

import { hrefAllowed, srcAllowed } from "./href.ts";

/** 本文のノード（Tiptap の JSON）。 */
export interface BodyNode {
  type: string;
  attrs?: Record<string, unknown>;
  content?: BodyNode[];
  marks?: { type: string; attrs?: Record<string, unknown> }[];
  text?: string;
}

type Overrides = Record<string, Component>;

const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);
const num = (v: unknown): number | undefined => (typeof v === "number" && Number.isFinite(v) ? v : undefined);

/** マークを外側から順にかける。 */
function withMarks(node: BodyNode, text: VNode | string, overrides: Overrides): VNode | string {
  let out: VNode | string = text;
  for (const mark of [...(node.marks ?? [])].reverse()) {
    const custom = overrides[`mark:${mark.type}`];
    if (custom) {
      const inner = out;
      out = h(custom, { mark }, { default: () => [inner] });
      continue;
    }
    switch (mark.type) {
      case "bold":
        out = h("strong", [out]);
        break;
      case "italic":
        out = h("em", [out]);
        break;
      case "strike":
        out = h("s", [out]);
        break;
      case "code":
        out = h("code", [out]);
        break;
      case "link": {
        const href = mark.attrs?.["href"];
        if (hrefAllowed(href)) {
          const target = str(mark.attrs?.["target"]);
          out = h("a", { href, target, rel: target === "_blank" ? "noopener noreferrer" : str(mark.attrs?.["rel"]) }, [out]);
        }
        break;
      }
      default:
        break;
    }
  }
  return out;
}

function renderNodes(nodes: BodyNode[] | undefined, overrides: Overrides): VNodeArrayChildren {
  return (nodes ?? []).map((n) => renderNode(n, overrides));
}

function renderNode(node: BodyNode, overrides: Overrides): VNode | string | null {
  const children = () => renderNodes(node.content, overrides);
  const custom = overrides[node.type];
  if (custom) return h(custom, { node }, { default: children });
  const a = node.attrs ?? {};
  switch (node.type) {
    case "text":
      return withMarks(node, node.text ?? "", overrides);
    case "doc":
      return h("div", children());
    case "paragraph":
      return h("p", children());
    case "heading": {
      const level = Math.min(Math.max(num(a["level"]) ?? 2, 1), 6);
      return h(`h${level}`, { id: str(a["id"]) }, children());
    }
    case "bulletList":
      return h("ul", children());
    case "orderedList":
      return h("ol", { start: num(a["start"]) }, children());
    case "listItem":
      return h("li", children());
    case "blockquote":
      return h("blockquote", children());
    case "codeBlock": {
      const lang = str(a["language"]);
      return h("pre", [h("code", { "data-language": lang }, children())]);
    }
    case "horizontalRule":
      return h("hr");
    case "hardBreak":
      return h("br");
    case "image": {
      const src = a["src"];
      if (!srcAllowed(src)) return null;
      const decorative = a["decorative"] === true;
      return h("img", {
        src,
        alt: decorative ? "" : (str(a["alt"]) ?? ""),
        width: num(a["width"]),
        height: num(a["height"]),
        loading: "lazy",
        decoding: "async",
      });
    }
    case "embed": {
      const src = a["src"];
      return /^https:\/\//i.test(String(src)) && hrefAllowed(src) ? h("p", [h("a", { href: src, rel: "noopener noreferrer" }, src)]) : null;
    }
    default:
      // 知らないノード（カスタムノードなど）は、子があれば子だけを描く。サイトの部品で置き換えられる
      return node.content ? h("div", { "data-cms-node": node.type }, children()) : null;
  }
}

export default defineComponent({
  name: "CmsBody",
  props: {
    /** 本文（配信APIが返す JSON）。 */
    doc: { type: Object as PropType<BodyNode | null | undefined>, default: null },
    /** ノード（`image` など）やマーク（`mark:link` など）を描く、サイトの部品。 */
    components: { type: Object as PropType<Overrides>, default: () => ({}) },
  },
  setup(props) {
    return () => {
      const doc = props.doc;
      if (!doc || typeof doc !== "object") return null;
      return renderNode(doc as BodyNode, props.components);
    };
  },
});
