// 管理APIが返す、スキーマと記事の形（server/src/schema/manifest.rs、server/src/content/*.rs）。
import type { JSONContent } from "@tiptap/core";
import type { ManifestBody } from "@new-cms/editor-schema";

export type FieldType = "text" | "slug" | "richtext" | "reference";

export interface Field {
  name: string;
  type: FieldType;
  required: boolean;
  referenceTo?: string;
  multiple: boolean;
  description?: string;
}

export interface ContentType {
  name: string;
  urlTemplate: string;
  description?: string;
  fields: Field[];
}

export interface Manifest {
  locales: string[];
  defaultLocale: string;
  body: ManifestBody;
  contentTypes: ContentType[];
}

/** GET /workspaces/{ws}/schema */
export interface SchemaInfo {
  template: string;
  version: number | null;
  state: "none" | "ok" | "pending";
  /** 適用済みのマニフェスト（まだなければ null）。 */
  manifest: Manifest | null;
}

export interface WorkspaceInfo {
  id: string;
  name: string;
  template: string;
  role: "admin" | "editor" | "author" | "viewer" | null;
  schemaVersion: number | null;
  schemaState: "none" | "ok" | "pending";
}

export type FieldValue = string | string[] | JSONContent | null;

export interface EntrySummary {
  id: string;
  contentType: string;
  locale: string;
  translationGroupId: string;
  revision: number;
  published: boolean;
  updatedAt: string;
  fields: Record<string, FieldValue>;
}

export interface Draft {
  id: string;
  contentType: string;
  locale: string;
  translationGroupId: string;
  revision: number;
  publishedVersionId: string | null;
  createdAt: string;
  updatedAt: string;
  fields: Record<string, FieldValue>;
}

export interface SectionChange {
  field: string | null;
  anchor: string | null;
  title: string | null;
  kind: "changed" | "added" | "removed";
  numbersOrLinksChanged: boolean;
}

/** GET .../entries/{id}/publish-check */
export interface PublishCheck {
  revision: number;
  published: boolean;
  canCorrect: boolean;
  problems: string[];
  imagesMissingAlt: string[];
  url: string | null;
  previousUrl: string | null;
  urlTaken: boolean;
  comparedWith: number | null;
  sections: SectionChange[] | null;
}

export interface Published {
  versionId: string;
  number: number;
  url: string;
  isCorrection: boolean;
  previousUrl: string | null;
}

export interface MediaInfo {
  id: string;
  url: string;
  contentType: string;
  bytes: number;
  width: number;
  height: number;
}

/** 一覧に出す記事の名前（最初の1行の文字列のフィールド）。 */
export function entryTitle(ct: ContentType | undefined, fields: Record<string, FieldValue>): string | null {
  const field = ct?.fields.find((f) => f.type === "text");
  const value = field ? fields[field.name] : null;
  return typeof value === "string" && value.trim() ? value : null;
}

/** 公開・非公開・削除ができるか（管理者・編集者とサーバー管理者。Q77, Q200）。 */
export function canPublish(w: WorkspaceInfo | null | undefined): boolean {
  return !!w && (w.role === null || w.role === "admin" || w.role === "editor");
}

/** 記事を作り、下書きを書けるか（執筆者以上とサーバー管理者。Q192）。 */
export function canEdit(w: WorkspaceInfo | null | undefined): boolean {
  return !!w && w.role !== "viewer";
}
