// 約束のテストで使うテンプレート（スキーマ）を、実装の書式によらない形で定義する。
// ドライバーが、実際のテンプレートのファイル（JSONとTiptapの拡張機能）に組み立てる。
// テスト環境の準備では BASE_TEMPLATE をデプロイしておく（README）。

/** スキーマ情報には出てよいが、devNote ではない目印。 */
export const DESCRIPTION_MARKER = "DESC-MARKER-0b7e2c";
/** 配信APIとスキーマ情報のAPIに決して出てはいけない目印（INV-SCH-01）。 */
export const DEVNOTE_MARKER = "DEVNOTE-MARKER-7f3a91";

export type FieldType = "text" | "slug" | "richtext" | "reference";

/**
 * 組み込みJSで動く関数（URL生成関数・フックなど）が、権限として宣言できるホスト関数（04-hooks.md、Q11）。
 * - readContent: 同じワークスペースのコンテンツを読む
 */
export type ScriptPermission = "readContent";

/** 組み込みJSで動く関数。source は support/scripts.ts の部品で組み立てる。 */
export interface ScriptSpec {
  source: string;
  /** 権限として宣言するホスト関数。宣言していないものは呼べない（INV-HOOK-02）。 */
  permissions: ScriptPermission[];
}

export interface FieldSpec {
  name: string;
  type: FieldType;
  required?: boolean;
  /** 参照フィールドの参照先のコンテンツタイプ（翻訳グループを指す）。 */
  referenceTo?: string;
  multiple?: boolean;
  description?: string;
  devNote?: string;
}

export interface ContentTypeSpec {
  name: string;
  fields: FieldSpec[];
  /** ロケールごとの公開URLのテンプレート。`{locale}` と `{slug}` を使える。 */
  urlTemplate: string;
  /** URL生成関数（M2、Q38）。あればテンプレートの代わりに使う。 */
  urlFunction?: ScriptSpec;
  /** 一覧のフィールドで1回に取得できる件数の上限。 */
  listLimit: number;
  description?: string;
  devNote?: string;
}

export interface TemplateSpec {
  name: string;
  locales: string[];
  defaultLocale: string;
  contentTypes: ContentTypeSpec[];
  /**
   * 本文で使えるノード。名前はTiptapの標準の名前（StarterKitとLink）に、
   * CMSの画像（`image`）と埋め込み（`embed`）を加えたもの。
   */
  bodyNodes: string[];
  bodyMarks: string[];
  /** 埋め込みを許す提供元のホスト名。 */
  embedProviders: string[];
}

export const BASE_TEMPLATE: TemplateSpec = {
  name: "inv-base",
  locales: ["ja", "en", "fr"],
  defaultLocale: "ja",
  bodyNodes: ["doc", "paragraph", "text", "heading", "bulletList", "listItem", "image", "embed"],
  bodyMarks: ["bold", "link"],
  embedProviders: ["www.youtube.com"],
  contentTypes: [
    {
      name: "post",
      urlTemplate: "https://site.inv.test/{locale}/blog/{slug}",
      listLimit: 5,
      fields: [
        { name: "title", type: "text", required: true },
        { name: "slug", type: "slug", required: true },
        { name: "subtitle", type: "text" },
        { name: "summary", type: "text", description: DESCRIPTION_MARKER, devNote: DEVNOTE_MARKER },
        { name: "body", type: "richtext" },
        { name: "related", type: "reference", referenceTo: "post", multiple: true },
      ],
      devNote: DEVNOTE_MARKER,
    },
  ],
};

/** テンプレートを名前を変えて複製する（スキーマのテストで、他のテストに影響しないよう別のテンプレートを使う）。 */
export function cloneTemplate(name: string, edit?: (t: TemplateSpec) => void): TemplateSpec {
  const t = structuredClone(BASE_TEMPLATE);
  t.name = name;
  edit?.(t);
  return t;
}

export function postType(t: TemplateSpec): ContentTypeSpec {
  const post = t.contentTypes.find((c) => c.name === "post");
  if (!post) throw new Error("post がない");
  return post;
}
