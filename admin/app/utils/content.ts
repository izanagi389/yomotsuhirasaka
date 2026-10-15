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

/** GET .../entries/{id}/translation */
export interface TranslationInfo {
  groupId: string;
  locale: string;
  sourceEntryId: string | null;
  sourceLocale: string | null;
  reflectedSourceVersionId: string | null;
  members: { entryId: string; locale: string; published: boolean }[];
}

export interface VersionInfo {
  id: string;
  number: number;
  schemaVersion: number;
  publishedBy: string | null;
  publishedAt: string;
  url: string | null;
  current: boolean;
  isCorrection: boolean;
  affectedAnchors: string[] | null;
  affectsLead: boolean | null;
  anchors: string[];
  erased: boolean;
  erasedAt: string | null;
}

export interface VersionDetail extends VersionInfo {
  /** 抹消した版は null。 */
  fields: Record<string, FieldValue> | null;
}

/** GET .../versions/{vid}/erase-candidates */
export interface EraseCandidates {
  media: string[];
  otherLocales: { entryId: string; locale: string }[];
}

export interface PublishSettings {
  useForChatbot: boolean;
  includeInAiMaterials: boolean;
  optOutOfAiTraining: boolean;
}

/** 公開設定のスイッチの既定値（server/src/content/settings.rs）。 */
export const DEFAULT_PUBLISH_SETTINGS: PublishSettings = {
  useForChatbot: true,
  includeInAiMaterials: true,
  optOutOfAiTraining: false,
};

export interface UrlChange {
  from: string;
  to: string;
  versionId: string | null;
  changedAt: string;
}

export interface MediaDetail extends MediaInfo {
  createdAt: string;
  usage: { entryId: string; locale: string; inDraft: boolean; versions: number[] }[];
}

export interface Limits {
  mediaBytes: number;
  mediaBytesUsed: number;
}

/** 版の本文の抹消ができるか（管理者とサーバー管理者。Q134）。 */
export function canErase(w: WorkspaceInfo | null | undefined): boolean {
  return !!w && (w.role === null || w.role === "admin");
}

/** バイト数を読みやすく。 */
export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}
