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
  /** 記事のタイトルにするフィールド。 */
  titleField?: string;
  /** 確認期限の既定。 */
  review?: { intervalDays: number };
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
  /** 自分が持つ重い権限（1件の取得のときだけ）。表示の出し分けに使う。照合はサーバーが操作のたびに行う。 */
  permissions?: HeavyPermission[];
  /** 編集の引き継ぎを誰ができるか（1件の取得のときだけ）。 */
  takeOverPolicy?: "anyone" | "permission";
  /** プレビューを許すサイトのオリジン（1件の取得のときだけ）。 */
  previewOrigins?: string[];
  /** サイトのプレビュー用ルートのパス（1件の取得のときだけ）。 */
  previewPath?: string;
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

/** 保存した下書き。保存前のフックが失敗したとき（直さずに保存した）は、その種類が付く。 */
export interface SavedDraft extends Draft {
  hookFailure?: string;
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
  /** 原文に未確認の更新があれば、その情報（公開は止めない。Q162, Q164）。 */
  sourceUpdate: SourceUpdate | null;
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
/** 個人に追加で付与できる重い権限（Q76。server/src/access.rs）。 */
export type HeavyPermission = "viewQuestionLog" | "manageLlm" | "policySwitches" | "corrections" | "takeOverEditing";
export const HEAVY_PERMISSIONS: HeavyPermission[] = ["viewQuestionLog", "manageLlm", "policySwitches", "corrections", "takeOverEditing"];

/** 重い権限を持つか（ロールの既定か、個人への付与）。 */
export function hasPermission(w: WorkspaceInfo | null | undefined, p: HeavyPermission): boolean {
  return !!w?.permissions?.includes(p);
}

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
  /** 原文の更新（反映済みの版より後に元の記事の版がある）。なければ null。 */
  sourceUpdate: SourceUpdate | null;
  members: { entryId: string; locale: string; published: boolean }[];
}

export interface SourceUpdate {
  latestVersionId: string;
  /** 反映済みの版より後に「訂正として公開」した版がある。 */
  corrected: boolean;
}

/** GET .../entries/{id}/translation/source-diff */
export interface SourceDiff {
  sourceEntryId: string | null;
  fromVersionId: string | null;
  toVersionId: string | null;
  diff: {
    changedFields: string[];
    sections: { anchor: string | null; field?: string; title: string | null; kind: "changed" | "added" | "removed"; before: string | null; after: string | null }[];
  } | null;
  /** 差分を出せない理由。 */
  unavailable?: "no_source" | "no_version" | "not_reflected" | "erased";
}

/** 行単位の比較の1行。 */
export interface DiffLine {
  text: string;
  kind: "same" | "removed" | "added";
}

/**
 * 2つの文字列を行単位で比べる（最長共通部分列）。前の列（same と removed）と後の列（same と added）を返す。
 * セクションの中の比較に使うので、行数は多くない前提（多すぎるときは比べずに全部を変わった行とする）。
 */
export function lineDiff(before: string | null, after: string | null): { before: DiffLine[]; after: DiffLine[] } {
  const a = before === null ? [] : before.split("\n");
  const b = after === null ? [] : after.split("\n");
  if (a.length * b.length > 250_000) {
    return { before: a.map((text) => ({ text, kind: "removed" })), after: b.map((text) => ({ text, kind: "added" })) };
  }
  // lcs[i][j]: a[i..] と b[j..] の最長共通部分列の長さ
  const lcs = Array.from({ length: a.length + 1 }, () => new Array<number>(b.length + 1).fill(0));
  for (let i = a.length - 1; i >= 0; i--) {
    for (let j = b.length - 1; j >= 0; j--) {
      lcs[i]![j] = a[i] === b[j] ? lcs[i + 1]![j + 1]! + 1 : Math.max(lcs[i + 1]![j]!, lcs[i]![j + 1]!);
    }
  }
  const out = { before: [] as DiffLine[], after: [] as DiffLine[] };
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) {
      out.before.push({ text: a[i]!, kind: "same" });
      out.after.push({ text: b[j]!, kind: "same" });
      i++;
      j++;
    } else if (lcs[i + 1]![j]! >= lcs[i]![j + 1]!) {
      out.before.push({ text: a[i++]!, kind: "removed" });
    } else {
      out.after.push({ text: b[j++]!, kind: "added" });
    }
  }
  while (i < a.length) out.before.push({ text: a[i++]!, kind: "removed" });
  while (j < b.length) out.after.push({ text: b[j++]!, kind: "added" });
  return out;
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
  /** 訂正の説明（訂正として公開した版で、書いたときだけ）。 */
  correctionNote: string | null;
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

// ---------------------------------------------------------------------------
// 訂正の説明と鮮度（06-corrections-and-freshness.md）
// ---------------------------------------------------------------------------

/** GET/PUT /workspaces/{ws}/correction-templates の1件。 */
export interface CorrectionTemplate {
  id?: string;
  locale: string;
  /** choice: 選べる文、fallback: 説明を書いていないときの文（ロケールに1つ）。 */
  kind: "choice" | "fallback";
  text: string;
}

/** GET .../versions/{vid}/correction-note/history の1件。 */
export interface NoteEdit {
  text: string;
  editedBy: string | null;
  editedAt: string;
}

/** GET /workspaces/{ws}/corrections/unwritten の1件。 */
export interface UnwrittenCorrection {
  entryId: string;
  versionId: string;
  locale: string;
  title: string | null;
  correctedAt: string;
}

/** GET .../entries/{id}/freshness */
export interface FreshnessDetail {
  lastVerifiedAt: string | null;
  reviewDueAt: string | null;
  reviewExpired: boolean;
  reviewIntervalDays: number | null;
  reviewDisabled: boolean;
  defaultIntervalDays: number | null;
  history: { verifiedAt: string; verifiedBy: string | null; memo: string }[];
}

/** GET /workspaces/{ws}/reviews/expired の1件。 */
export interface ExpiredReview {
  entryId: string;
  locale: string;
  title: string | null;
  reviewDueAt: string;
}

/** レビュー依頼に出てくる人（公開できる人と依頼した本人だけ。それ以外は null）。server/src/http/admin_reviews.rs */
export interface ReviewPerson {
  userId: string;
  /** 表示名（未設定ならメールアドレス）。 */
  name: string;
  /** 同じ名前の人が並ぶときだけ。 */
  email?: string;
}

/** レビュー依頼の1件。 */
export interface ReviewRequest {
  id: string;
  entryId: string;
  state: "open" | "rejected" | "withdrawn" | "published";
  requestedBy: ReviewPerson | null;
  reviewer: ReviewPerson | null;
  assignedToMe: boolean;
  requestedRevision: number;
  changedSinceRequest: boolean;
  comment: string | null;
  createdAt: string;
  closedAt: string | null;
  closedBy: ReviewPerson | null;
}

/** GET .../entries/{id}/review-request */
export interface EntryReview {
  open: ReviewRequest | null;
  history: ReviewRequest[];
}

/** GET /workspaces/{ws}/review-requests */
export interface ReviewLists {
  waiting: { request: ReviewRequest; entry: EntrySummary }[];
  rejected: { request: ReviewRequest; entry: EntrySummary }[];
}

/** 人の名前（同じ名前の人にはメールアドレスを添える）。 */
export function personLabel(p: ReviewPerson | null | undefined, other: string): string {
  if (!p) return other;
  return p.email ? `${p.name} (${p.email})` : p.name;
}

/** 予約公開（server/src/http/admin_scheduling.rs）。 */
export interface ScheduleView {
  id: string;
  entryId: string;
  publishAt: string;
  draftRevision: number;
  scheduledBy: string;
  createdAt: string;
  state: "pending" | "published" | "cancelled" | "failed";
  reason: "cancelled" | "replaced" | "corrected" | "published" | "deleted" | null;
  /** 公開できなかった理由（やり直している間と、失敗したとき）。 */
  lastError: string | null;
  nextAttemptAt: string | null;
  closedAt: string | null;
  closedBy: string | null;
  /** 予約の後に下書きが編集されたか。 */
  draftChanged: boolean;
  /** 予約した人（いまも公開できる人のときだけ）。 */
  scheduledByPerson: ReviewPerson | null;
}

/** GET .../entries/{id}/schedule */
export interface EntrySchedule {
  open: ScheduleView | null;
  latest: ScheduleView | null;
}

/** GET /workspaces/{ws}/schedules */
export interface ScheduleLists {
  retrying: { schedule: ScheduleView; entry: EntrySummary }[];
  attention: { schedule: ScheduleView; entry: EntrySummary }[];
}

/** 「やること」の1件（GET /workspaces/{ws}/todo。server/src/http/admin_todo.rs）。並びは優先度の順。 */
export interface TodoItem {
  kind:
    | "scheduleFailed"
    | "scheduleRetrying"
    | "reviewWaiting"
    | "unwrittenCorrection"
    | "reviewRejected"
    | "sourceCorrected"
    | "reviewExpired"
    | "sourceUpdated"
    | "scheduleCancelled";
  entry: EntrySummary;
  /** 並べるときに使った時刻（依頼・差し戻し・予約・訂正・確認期限・原文の公開）。 */
  at: string;
  request?: ReviewRequest;
  schedule?: ScheduleView;
  versionId?: string;
}

/** POST .../entries/{id}/preview-tokens */
export interface PreviewToken {
  id: string;
  token: string;
  expiresAt: string;
  /** 下書きから作った公開URL（作れなければ null）。 */
  url: string | null;
}
