// ドライバーの一部: テストのテンプレート（template.ts の TemplateSpec）を、サーバーのデプロイのAPIが受け取る
// マニフェスト（server/src/schema/manifest.rs）に組み立てる。
// 本番ではCLIが、テンプレートのJSONとTiptapの拡張機能から同じ形を作る。ここでは、Tiptapの標準の拡張機能
// （StarterKit と Link）と、CMSの画像（image）・埋め込み（embed）の定義を、名前から引いて書き出す。

import type { ScriptPermission, ScriptSpec, TemplateSpec } from "./template.js";

interface NodeDef {
  children: string[];
  attrs: string[];
  marks: boolean;
}

const BLOCKS = ["paragraph", "heading", "bulletList", "orderedList", "blockquote", "codeBlock", "horizontalRule", "image", "embed"];
const INLINE = ["text", "hardBreak"];

/** Tiptapの拡張機能の定義（content式を、許す子ノードの一覧に簡略化したもの）。 */
const NODE_DEFS: Record<string, NodeDef> = {
  doc: { children: BLOCKS, attrs: [], marks: false },
  paragraph: { children: INLINE, attrs: [], marks: true },
  heading: { children: INLINE, attrs: ["level"], marks: true },
  bulletList: { children: ["listItem"], attrs: [], marks: false },
  orderedList: { children: ["listItem"], attrs: ["start"], marks: false },
  listItem: { children: ["paragraph", "bulletList", "orderedList"], attrs: [], marks: false },
  blockquote: { children: BLOCKS, attrs: [], marks: false },
  codeBlock: { children: ["text"], attrs: ["language"], marks: false },
  horizontalRule: { children: [], attrs: [], marks: false },
  hardBreak: { children: [], attrs: [], marks: false },
  text: { children: [], attrs: [], marks: false },
  image: { children: [], attrs: ["mediaId", "alt", "decorative"], marks: false },
  embed: { children: [], attrs: ["src"], marks: false },
};

const MARK_ATTRS: Record<string, string[]> = {
  bold: [],
  italic: [],
  strike: [],
  code: [],
  link: ["href", "target", "rel"],
};

// ホスト関数の権限の名前（server/src/script/ のホスト関数の宣言に合わせる）
const PERMISSION_NAMES: Record<ScriptPermission, string> = {
  readContent: "content.read",
  fetch: "http.fetch",
};

// 本番ではCLIがTSをJSに変換して、ソースのままマニフェストに入れる（バイトコードは受け取らない）
const toScript = (s: ScriptSpec) => ({
  source: s.source,
  permissions: s.permissions.map((p) => PERMISSION_NAMES[p]),
  allowedHosts: s.allowedHosts,
});

export function toManifest(t: TemplateSpec): unknown {
  const present = new Set(t.bodyNodes);
  return {
    locales: t.locales,
    defaultLocale: t.defaultLocale,
    body: {
      nodes: t.bodyNodes.map((name) => {
        const def = NODE_DEFS[name];
        if (!def) throw new Error(`ドライバーが知らない本文のノードです: ${name}（tests/invariants/support/manifest.ts）`);
        return { name, children: def.children.filter((c) => present.has(c)), attrs: def.attrs, marks: def.marks };
      }),
      marks: t.bodyMarks.map((name) => {
        const attrs = MARK_ATTRS[name];
        if (!attrs) throw new Error(`ドライバーが知らない本文のマークです: ${name}（tests/invariants/support/manifest.ts）`);
        return { name, attrs };
      }),
      embedProviders: t.embedProviders,
    },
    contentTypes: t.contentTypes.map((ct) => ({
      name: ct.name,
      urlTemplate: ct.urlTemplate,
      urlFunction: ct.urlFunction ? toScript(ct.urlFunction) : undefined,
      hooks: ct.hooks ? Object.fromEntries(Object.entries(ct.hooks).map(([kind, s]) => [kind, toScript(s)])) : undefined,
      listLimit: ct.listLimit,
      description: ct.description,
      devNote: ct.devNote,
      fields: ct.fields.map((f) => ({
        name: f.name,
        type: f.type,
        required: f.required ?? false,
        referenceTo: f.referenceTo,
        multiple: f.multiple ?? false,
        description: f.description,
        devNote: f.devNote,
      })),
    })),
  };
}
