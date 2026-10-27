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
 * - fetch: 外部へHTTPで送る（非同期のフックだけ。送信先は allowedHosts で宣言する）
 */
export type ScriptPermission = "readContent" | "fetch";

/** 組み込みJSで動く関数。source は support/scripts.ts の部品で組み立てる。 */
export interface ScriptSpec {
  source: string;
  /** 権限として宣言するホスト関数。宣言していないものは呼べない（INV-HOOK-02）。 */
  permissions: ScriptPermission[];
  /** fetch で送ってよいホスト名（またはIPアドレス）。 */
  allowedHosts?: string[];
}

/**
 * フックの種類（04-hooks.md、Q12）。
 * - 同期（結果を待つ。外部と通信できない）: beforeSave（下書きの保存の前。フィールドの直しを返す）、
 *   beforePublish（公開の前。公開を止める理由を返せる）
 * - 非同期（後から動く。リトライ付き）: afterSave（保存が止まってから記事ごとに1回）、afterPublish、afterUnpublish
 */
export type HookKind = "beforeSave" | "beforePublish" | "afterSave" | "afterPublish" | "afterUnpublish";

export interface FieldSpec {
  name: string;
  type: FieldType;
  required?: boolean;
  /** 参照フィールドの参照先のコンテンツタイプ（翻訳グループを指す）。 */
  referenceTo?: string;
  multiple?: boolean;
  description?: string;
  devNote?: string;
  /**
   * 検索・質問応答の対象にするか（M3、08-search-and-related.md、Q16）。既定は対象外。
   * 対象のフィールドの内容から、チャンクと埋め込みを作る。
   */
  searchable?: boolean;
}

export interface ContentTypeSpec {
  name: string;
  fields: FieldSpec[];
  /** ロケールごとの公開URLのテンプレート。`{locale}` と `{slug}` を使える。 */
  urlTemplate: string;
  /** URL生成関数（M2、Q38）。あればテンプレートの代わりに使う。 */
  urlFunction?: ScriptSpec;
  /** フック（M2）。種類ごとに1つ。 */
  hooks?: Partial<Record<HookKind, ScriptSpec>>;
  /** 一覧のフィールドで1回に取得できる件数の上限。 */
  listLimit: number;
  /**
   * AI向けのMarkdown版に載せるもの（M2、07-ai-delivery.md、Q35）。ここに挙げていないフィールドは載せない。
   * 書いていないコンテンツタイプの記事は、AI向けの素材（Markdown版・目録）にならない。
   * - title: 見出し（`#`）にするフィールド
   * - meta: 冒頭のメタデータ（front matter）に載せるフィールド
   * - fields: 本文として、この順に載せるフィールド
   * - relations: 関係として載せる参照フィールド（関係の種類はフィールドの名前、相手は記事のURL）
   */
  aiMarkdown?: { title: string; meta?: string[]; fields?: string[]; relations?: string[] };
  description?: string;
  devNote?: string;
}

/** カスタムノードの属性の型。 */
export type CustomAttrType = "string" | "number" | "boolean";

/**
 * カスタムノード（M2、Q36, Q37）。テンプレートに宣言的に書き、Tiptapの拡張機能は宣言から作る。
 * 変換関数のソースは support/scripts.ts の部品で組み立てる。
 */
export interface CustomNodeSpec {
  /** ノードの名前（英字で始まる英数字）。 */
  name: string;
  /** インライン（段落の子）か。既定はブロック（doc の子）。 */
  inline?: boolean;
  attrs: { name: string; type: CustomAttrType }[];
  /** 子として許すノード。なければ子を持たない。 */
  children?: string[];
  /** Markdownへの変換関数（必須。ないものはデプロイが拒否される: INV-NODE-01）。 */
  toMarkdown?: string;
  /** HTMLへの変換関数（任意。出力は無害化される: INV-CNV-01）。 */
  toHtml?: string;
  /**
   * カスタムノードの宣言を通さず、マニフェストの本文のノード（bodyNodes と同じところ）に直接書く。
   * そこには変換関数を書く場所がない。CLIを通さない手書きのマニフェストを想定した確認に使う。
   */
  asPlainBodyNode?: boolean;
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
  /** カスタムノード（M2）。 */
  customNodes?: CustomNodeSpec[];
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

/**
 * 記事の本文（body）を検索・質問応答の対象にしたテンプレート（M3）。BASE_TEMPLATE は対象のフィールドを持たないので、
 * 埋め込みを作る約束のテストは、これをデプロイしたインスタンスで確かめる。
 */
export const SEARCHABLE_TEMPLATE: TemplateSpec = cloneTemplate("inv-searchable", (t) => {
  for (const f of postType(t).fields) if (f.name === "body") f.searchable = true;
});
