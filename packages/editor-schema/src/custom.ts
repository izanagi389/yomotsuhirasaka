// カスタムノード: テンプレートの宣言（template.json の body.customNodes）から Tiptap の拡張機能を作る。
// 管理画面のエディタと、マニフェストを作るCLIの両方が、この関数を使う（宣言が正。拡張機能は宣言から決まる）。
// - ブロックは block のグループ（本文の直下、引用やリストの項目の中など、ブロックを置ける所ならどこでも置ける）
// - インラインは inline のグループ（段落や見出しの中）
// - 子のないノードは atom（ひとまとまり）。子があれば、宣言した子の一覧を content 式にする
// - 属性の初期値は null。コピーと貼り付けのため、HTMLでは `div[data-node=名前]`（インラインは span）と
//   `data-attrs`（属性のJSON）で読み書きする。読むときは、宣言した型の値だけを取り込む

import { Node } from "@tiptap/core";

export type CustomAttrType = "string" | "number" | "boolean";

export interface CustomAttrDecl {
  name: string;
  type: CustomAttrType;
  /** 編集者向けの説明。 */
  description?: string;
}

/** カスタムノードの宣言（server/src/schema/manifest.rs の CustomNode のうち、エディタが使う部分）。 */
export interface CustomNodeDecl {
  name: string;
  inline?: boolean;
  attrs?: CustomAttrDecl[];
  children?: string[];
  /** 編集者向けの説明。 */
  description?: string;
}

const ofType = (type: CustomAttrType, v: unknown): boolean =>
  v === null || (type === "string" ? typeof v === "string" : type === "number" ? typeof v === "number" && Number.isFinite(v) : typeof v === "boolean");

/** 宣言から、Tiptap のノードの拡張機能を作る。 */
export function customNodeExtension(decl: CustomNodeDecl) {
  const inline = decl.inline === true;
  const children = decl.children ?? [];
  const attrs = decl.attrs ?? [];
  const tag = inline ? "span" : "div";
  return Node.create({
    name: decl.name,
    group: inline ? "inline" : "block",
    inline,
    atom: children.length === 0,
    selectable: true,
    draggable: !inline,
    ...(children.length > 0 ? { content: `(${children.join(" | ")})*` } : {}),
    addAttributes() {
      return Object.fromEntries(attrs.map((a) => [a.name, { default: null, rendered: false }]));
    },
    parseHTML() {
      return [
        {
          tag: `${tag}[data-node="${decl.name}"]`,
          getAttrs: (el) => {
            let parsed: unknown;
            try {
              parsed = JSON.parse((el as HTMLElement).getAttribute("data-attrs") ?? "{}");
            } catch {
              parsed = {};
            }
            const values = typeof parsed === "object" && parsed !== null ? (parsed as Record<string, unknown>) : {};
            return Object.fromEntries(attrs.filter((a) => ofType(a.type, values[a.name] ?? null)).map((a) => [a.name, values[a.name] ?? null]));
          },
        },
      ];
    },
    renderHTML({ node }) {
      const out = { "data-node": decl.name, "data-attrs": JSON.stringify(node.attrs) };
      return children.length > 0 ? [tag, out, 0] : [tag, out];
    },
  });
}

/** 宣言の一覧から、拡張機能の一覧を作る。 */
export function customNodeExtensions(decls: CustomNodeDecl[]) {
  return decls.map(customNodeExtension);
}
