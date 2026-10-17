// テンプレートの関数（URL生成関数・フック・カスタムノードの変換関数）を書くための型。
// 使い方: `import type { ... } from "@new-cms/cli/types";`（型だけなので、CLIがJSにするときに消える）
// 受け取る値と返す値は docs/decisions/04-hooks.md と server/src/schema/manifest.rs の Hooks・CustomNode を参照。

/** 本文のノード（Tiptap の JSON）。 */
export interface CustomNode {
  type: string;
  attrs?: Record<string, string | number | boolean | null>;
  content?: CustomNode[];
  marks?: { type: string; attrs?: Record<string, unknown> }[];
  text?: string;
}

/** 変換関数に渡すもの。 */
export interface ConversionContext {
  format: "markdown" | "html";
  /** 子ノードを、同じ形式に変換したもの。 */
  children(): string;
}

/** カスタムノードの変換関数（toMarkdown は必須、toHtml は任意。toHtml の出力は無害化される）。 */
export type ConversionFunction = (node: CustomNode, ctx: ConversionContext) => string;

/** 記事（フックが受け取るもの）。 */
export interface Entry {
  id: string;
  contentType: string;
  locale: string;
  fields: Record<string, unknown>;
}

/** URL生成関数が受け取るもの。返すのは http(s) の絶対URL。 */
export interface UrlInput extends Entry {
  slug: string;
}
export type UrlFunction = (entry: UrlInput) => string;

/** 保存前のフック。直すフィールドを返す（保存は止められない）。 */
export type BeforeSaveHook = (input: { entry: Entry }) => { fields?: Record<string, unknown> } | null | undefined;

/** 公開前のフック。公開を止める理由を返す。 */
export type BeforePublishHook = (input: { entry: Entry }) => { errors?: { field?: string; message: string }[] } | null | undefined;

/** 非同期のフックが受け取るイベント。 */
export interface HookEvent {
  /** 受け手が同じイベントを二度処理しないための目印。 */
  id: string;
  type: "entry.saved" | "entry.published" | "entry.updated" | "entry.corrected" | "entry.unpublished" | "entry.deleted";
  workspaceId: string;
  entryId: string;
  locale: string;
  versionId: string | null;
  url: string | null;
}

/** 非同期のフック（afterSave・afterPublish・afterUnpublish）。返した値は使われない。 */
export type AfterHook = (input: { event: HookEvent; entry: Entry | null }) => void;

/** 権限として宣言したときだけ置かれるホスト関数。 */
export interface Cms {
  /** "content.read": 同じワークスペースの公開中の記事。なければ null。 */
  content?: { get(id: string): Entry | null };
  /** "http.fetch"（非同期のフックだけ）: allowedHosts の宛先に送る。失敗は例外。 */
  http?: {
    fetch(
      url: string,
      options?: { method?: "GET" | "POST" | "PUT" | "PATCH" | "DELETE"; headers?: Record<string, string>; body?: string },
    ): { status: number; body: string };
  };
}

declare global {
  const cms: Cms;
}
