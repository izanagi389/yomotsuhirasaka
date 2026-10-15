// CMSが意味を知っているノード（画像・埋め込み）と、見出しのアンカーID。
// 画面の描き方（Vueのノードビュー）は管理画面が `extend` で足す。ここではスキーマ（名前・属性・親子）だけを決める。

import { Extension, Node } from "@tiptap/core";
import type { Node as PMNode } from "@tiptap/pm/model";
import { Plugin, PluginKey } from "@tiptap/pm/state";

/** 見出しのアンカーIDの属性の名前（サーバーの body.rs の ANCHOR_ATTR と同じ）。 */
export const ANCHOR_ATTR = "id";

/** サーバーが受け付けるアンカーIDの形（body.rs の valid_anchor と同じ）。 */
export function isValidAnchor(id: unknown): id is string {
  return typeof id === "string" && id.length >= 1 && id.length <= 64 && /^[A-Za-z0-9_-]+$/.test(id);
}

/** 仮のアンカーID。サーバーの形（`h-` と英小文字・数字10文字）にそろえる。 */
export function newAnchor(): string {
  const alphabet = "0123456789abcdefghijklmnopqrstuvwxyz";
  const bytes = new Uint8Array(10);
  crypto.getRandomValues(bytes);
  return "h-" + Array.from(bytes, (b) => alphabet[b % alphabet.length]).join("");
}

/**
 * アンカーIDを振り直す見出しの位置と新しいID。IDのない見出しと、本文の中で2つ目以降に出てきた同じIDの見出しが対象
 * （サーバーの assign_anchors と同じ規則）。
 */
export function anchorFixes(doc: PMNode): { pos: number; id: string }[] {
  const seen = new Set<string>();
  const fixes: { pos: number; id: string }[] = [];
  doc.descendants((node, pos) => {
    if (node.type.name !== "heading") return true;
    const id: unknown = node.attrs[ANCHOR_ATTR];
    if (isValidAnchor(id) && !seen.has(id)) {
      seen.add(id);
    } else {
      let fresh = newAnchor();
      while (seen.has(fresh)) fresh = newAnchor();
      seen.add(fresh);
      fixes.push({ pos, id: fresh });
    }
    return false;
  });
  return fixes;
}

/**
 * 見出しのアンカーID（Q28, Q136）。最終的にはサーバーが保存時に決めるが、エディタでも仮のIDを振る。
 * IDのない見出しと、本文の中で2つ目以降に出てきた同じIDの見出しに、新しいIDを振る（サーバーと同じ規則）。
 */
export const HeadingAnchors = Extension.create({
  name: "headingAnchors",

  addGlobalAttributes() {
    return [
      {
        types: ["heading"],
        attributes: {
          [ANCHOR_ATTR]: {
            default: null,
            // 見出しを分割したときに、同じIDを引き継がない
            keepOnSplit: false,
            parseHTML: (el) => el.getAttribute("data-anchor-id"),
            // 管理画面の中で要素の id がぶつからないよう、data 属性に書く
            renderHTML: (attrs) => (attrs[ANCHOR_ATTR] ? { "data-anchor-id": attrs[ANCHOR_ATTR] } : {}),
          },
        },
      },
    ];
  },

  addProseMirrorPlugins() {
    return [
      new Plugin({
        key: new PluginKey("headingAnchors"),
        appendTransaction: (transactions, _old, state) => {
          if (!transactions.some((t) => t.docChanged)) return null;
          const fixes = anchorFixes(state.doc);
          if (fixes.length === 0) return null;
          const tr = state.tr;
          for (const f of fixes) tr.setNodeAttribute(f.pos, ANCHOR_ATTR, f.id);
          return tr.setMeta("addToHistory", false);
        },
      }),
    ];
  },
});

/**
 * 画像（Q75）。画像の実体はメディアのIDで指す（URLは本文に書かない）。代替テキストか「装飾画像」の印のどちらかがないと公開できない。
 * 外から貼り付けた画像（`data-media-id` のない img）は取り込まない。
 */
export const CmsImage = Node.create({
  name: "image",
  group: "block",
  atom: true,
  draggable: true,
  selectable: true,

  addAttributes() {
    return {
      mediaId: { default: null, parseHTML: (el) => el.getAttribute("data-media-id") },
      alt: { default: null, parseHTML: (el) => el.getAttribute("alt") },
      decorative: { default: false, parseHTML: (el) => el.getAttribute("data-decorative") === "true" },
    };
  },

  parseHTML() {
    return [{ tag: "img[data-media-id]" }];
  },

  renderHTML({ node }) {
    return [
      "img",
      {
        "data-media-id": node.attrs.mediaId,
        alt: node.attrs.decorative ? "" : (node.attrs.alt ?? ""),
        "data-decorative": node.attrs.decorative ? "true" : null,
      },
    ];
  },
});

/** 埋め込み（Q97）。許可した提供元の https のURLだけをサーバーが受け付ける。 */
export const CmsEmbed = Node.create({
  name: "embed",
  group: "block",
  atom: true,
  draggable: true,
  selectable: true,

  addAttributes() {
    return {
      src: { default: null, parseHTML: (el) => el.getAttribute("data-embed-src") },
    };
  },

  parseHTML() {
    return [{ tag: "div[data-embed-src]" }];
  },

  renderHTML({ node }) {
    return ["div", { "data-embed-src": node.attrs.src }];
  },
});
