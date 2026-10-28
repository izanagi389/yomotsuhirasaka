// ドライバー: 約束のテストと、実際のCMSのAPI・設定・DB・CLIのつなぎ目。
//
// 約束のテストは「何を確かめるか」だけを書き、管理APIのURLや設定の書式などの
// 実装の細部はこのファイルに閉じ込める。管理APIの形が決まるまで、各関数は
// NotImplementedError を投げる。実装を担当するAIが、APIを作るのと同時に中身を書く。
//
// このファイルの規則（変更は持ち主のレビューを受ける。CODEOWNERS）:
// - 中身は「HTTPの呼び出し・設定の書き出し・プロセスの起動・応答の読み取り」だけにする。合否の判定を書かない。
// - 失敗を成功に見せかけない。セットアップ用の関数（名前が Raw で終わらないもの）は、
//   CMSが失敗を返したら例外を投げる。
// - 生の応答を返す関数（名前が Raw で終わるもの、adminOperations）は、応答をそのまま返し、加工しない。
// - 読み取り用の関数（parse...、read...）は、応答の書式を読むだけにし、値を補ったり捨てたりしない。

import type { HttpResponse } from "./http.js";
import type { TiptapDoc, TiptapNode } from "./body.js";
import type { TemplateSpec } from "./template.js";
import { SECRET_SETTING_NAMES, rotateEncryptionKeyWithCli, runServerCli, startServerProcess, validServerConfigFromEnv } from "./server-process.js";
import { connectAdmin } from "./admin-client.js";
import { listMail } from "./smtp-sink.js";
import { ident, withClient } from "./db.js";
import { env } from "./env.js";
import { randomUUID } from "node:crypto";
import { spawn } from "node:child_process";
import { copyFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { BASE_TEMPLATE } from "./template.js";

export class NotImplementedError extends Error {
  constructor(what: string) {
    super(`ドライバーが未実装です: ${what}（tests/invariants/support/driver.ts）`);
    this.name = "NotImplementedError";
  }
}

const todo = (what: string): never => {
  throw new NotImplementedError(what);
};

// ---------------------------------------------------------------------------
// 利用者・セッション・認証
// ---------------------------------------------------------------------------

export interface Credentials {
  email: string;
  password: string;
  /** 多要素認証（TOTP）の秘密。設定していない利用者は undefined。 */
  totpSecret?: string;
}

/**
 * 個人に追加で付与できる重い権限（11-permissions-and-audit.md、Q76）。
 * - viewQuestionLog: 質問ログの閲覧（どのロールにも既定では付かない。M4で使う）
 * - manageLlm: LLMの接続設定と予算の変更（M3以降で使う）
 * - policySwitches: サイトの方針に関わるスイッチ（学習への利用の拒否、外部AI向けの素材に含める、チャットボットの対象）
 * - corrections: 訂正としての公開と、確認済みの記録
 * - takeOverEditing: 編集の引き継ぎ
 */
/** サーバー管理者が決めるワークスペースの上限。書いた項目だけを変える。 */
export interface WorkspaceLimits {
  mediaBytes?: number;
  /** 埋め込みの作成の予算の天井（トークン数）。null は設定ファイルの既定値。 */
  embeddingTokensPerMonth?: number | null;
  embeddingTokensPerDay?: number | null;
}

/** ワークスペースの埋め込みの作成の予算（トークン数）。null は上限なし（サーバー管理者の天井の範囲で）。 */
export interface EmbeddingBudget {
  tokensPerMonth: number | null;
  tokensPerDay: number | null;
}

/** 埋め込みモデルの切り替えの、ワークスペースごとの状態（M3、08-search-and-related.md「モデルの変更」）。 */
export type EmbeddingSwitchState =
  /** 設定に新しいモデルがあるが、サーバー管理者がまだ開始していない。 */
  | "notStarted"
  /** 新しいモデルの埋め込みを作っている（検索は旧モデル）。 */
  | "inProgress"
  /** 予算が尽きて止まっている（検索は旧モデル）。 */
  | "paused"
  /** 新しいモデルに切り替わった。 */
  | "switched"
  /** 使っているモデルが設定にない（ベクトル検索は使えない）。 */
  | "needsMigration";

/** 埋め込みモデルの切り替えの見積もりと進み具合。 */
export interface EmbeddingSwitchStatus {
  /** 設定の新しいモデル（なければ null）。 */
  nextModel: string | null;
  workspaces: {
    workspaceId: string;
    /** 新しいモデルの埋め込みを、まだ作っていないチャンクの数。 */
    chunksToEmbed: number;
    /** そのトークン数の見積もり（多めに数える）。 */
    estimatedTokens: number;
    state: EmbeddingSwitchState;
  }[];
}

export type HeavyPermission = "viewQuestionLog" | "manageLlm" | "policySwitches" | "corrections" | "takeOverEditing";

export interface Member extends Credentials {
  userId: string;
}

export interface Session {
  /** ブラウザが送り返すCookie（名前 → 値）。 */
  cookies: Map<string, string>;
  /** 状態を変える操作に付けるCSRFトークン（ログインが完了していなければ空文字）。 */
  csrfToken: string;
  /** ログインの途中も含め、受け取ったすべての Set-Cookie ヘッダー。 */
  setCookieHeaders: string[];
}

/** 管理APIへの認証の付け方。 */
export type Auth =
  | { kind: "none" }
  | { kind: "session"; session: Session; csrf: "valid" | "missing" | { token: string } }
  | { kind: "bearer"; token: string };

export const asSession = (session: Session): Auth => ({ kind: "session", session, csrf: "valid" });
export const asBearer = (token: string): Auth => ({ kind: "bearer", token });

/** リクエストに付け足すもの。 */
export interface RequestExtras {
  /** URLのクエリに足すパラメーター。 */
  query?: Record<string, string>;
  headers?: Record<string, string>;
  /**
   * 接続元のIPアドレス。テスト環境では、テストの実行環境を信頼するプロキシとして設定しておき、
   * `X-Forwarded-For` で渡す（Q106）。回数制限が他のテストに波及しないよう、テストごとに別の値を使う。
   */
  clientIp?: string;
}

export interface LoginOptions {
  /** ログインの前から持っているCookie（新しい端末の判定用のCookieなど）。 */
  cookies?: Map<string, string>;
  extras?: RequestExtras;
}

export type WorkspaceRole = "admin" | "editor" | "author" | "viewer";

/** 配信APIのキーのスコープ。"read" は公開版の読み取りだけ、"register" はクエリの登録。 */
export type ApiKeyScope = "read" | "register";

/** 個人トークンの範囲。"read" は読み取りだけ、"write" は本人のロールの範囲の書き込み、"deploy" はデプロイ（サーバー管理者だけ）。 */
export type TokenScope = "read" | "write" | "deploy";

export interface IssuedSecret {
  id: string;
  /** 発行時の応答にだけ含まれる秘密の値。 */
  secret: string;
  /** 発行時の応答（秘密の値を含む）。 */
  response: HttpResponse;
}

/** パスキー（ソフトウェアで作った認証器）。中身はドライバーが持つ。 */
export interface PasskeyHandle {
  readonly credentialId: string;
}

// ---------------------------------------------------------------------------
// 管理APIの操作の一覧
// ---------------------------------------------------------------------------

/** 操作の対象。テストごとに作った使い捨ての対象を渡す。 */
export interface OperationTarget {
  workspaceId: string;
  entryId: string;
  userId: string;
  apiKeyId: string;
}

/**
 * 管理APIの操作の一覧の1件。
 * 一覧には、ログインが必要な管理APIのすべての操作を載せる（新しい操作を作ったら、ここにも足す）。
 * ログインの前に使う操作（ログイン、リセットの要求、招待の承諾など）は載せない。
 */
export interface AdminOperation {
  /** 操作の名前（英語のキャメルケース）。 */
  name: string;
  /** 状態を変える操作か。 */
  mutating: boolean;
  /**
   * 操作の向き先。
   * - "self": ログイン中の本人に対する操作。target は使わない
   * - "workspace": target.workspaceId のワークスペースに対する操作
   * - "instance": インスタンス全体に対する操作（サーバー管理者向け）
   */
  scope: "self" | "workspace" | "instance";
  /** 1回呼び出して、応答をそのまま返す。 */
  call(auth: Auth, target: OperationTarget, extras?: RequestExtras): Promise<HttpResponse>;
}

// ---------------------------------------------------------------------------
// 記事・版・翻訳
// ---------------------------------------------------------------------------

export interface EntryInput {
  title: string;
  body: TiptapDoc;
  /** 省略時はドライバーが一意な値を付ける。 */
  slug?: string;
  /** 省略時はテンプレートの既定のロケール。 */
  locale?: string;
  /** その他のフィールド（`subtitle`、`summary`、`related`（翻訳グループIDの配列）など）。 */
  fields?: Record<string, unknown>;
}

export interface Draft {
  revision: string;
  title: string;
  body: TiptapDoc;
  slug: string;
  fields: Record<string, unknown>;
}

export interface VersionInfo {
  id: string;
  /** 「訂正として公開」した版か。 */
  isCorrection: boolean;
  /** 本文を抹消した版か。 */
  erased: boolean;
}

export interface TranslationInfo {
  groupId: string;
  locale: string;
  /** 元の記事。元なしなら null。 */
  sourceEntryId: string | null;
  /** 反映済みとして記録した元の記事の版。元なしなら null。 */
  reflectedSourceVersionId: string | null;
  /** 元の記事に、反映済みの版より後の版がある（「原文が更新された」状態。M2）。 */
  sourceUpdated: boolean;
  /** 反映済みの版より後に、「訂正として公開」した版がある（M2）。 */
  sourceCorrected: boolean;
}

/** 「やること」の1件（M2）。kind は項目の種類。 */
export interface TodoItem {
  kind:
    | "scheduleFailed"
    | "scheduleRetrying"
    | "reviewWaiting"
    | "notification"
    | "unwrittenCorrection"
    | "reviewRejected"
    | "sourceCorrected"
    | "reviewExpired"
    | "sourceUpdated"
    | "scheduleCancelled";
  /** 記事の項目の記事（通知の項目は null）。 */
  entryId: string | null;
  /** 通知の項目の、監査ログの記録の操作の種類と対象のID（M2）。 */
  notification?: { action: string; targetIds: string[] };
}

/** 元の記事の、反映済みの版から今の版までの差分（M2）。 */
export interface SourceDiff {
  /** 比べた元の記事の版（反映済みの版と、今の版）。 */
  fromVersionId: string | null;
  toVersionId: string | null;
  /** 差分を出せたか（版の本文が抹消されていると出せない）。 */
  available: boolean;
  /** 本文以外で変わったフィールドの名前。 */
  changedFields: string[];
  /** 変わったセクション。before・after はそのセクションの文字列（ないほうは null）。 */
  sections: { anchor: string | null; kind: "changed" | "added" | "removed"; before: string | null; after: string | null }[];
}

/** リダイレクト一覧の1件。to は「消えた（410）」のとき null。 */
export interface RedirectRule {
  from: string;
  to: string | null;
  status: number;
}

export interface UrlChange {
  from: string;
  to: string;
}

// ---------------------------------------------------------------------------
// 配信API
// ---------------------------------------------------------------------------

/**
 * 配信APIのクエリの仕様。ドライバーが、テンプレートの命名に合わせたGraphQLの文に組み立てる。
 * fields を省略したときに返す項目は `id`・`title`・`slug` だけ（他のフィールドを使うクエリは fields で明示する）。
 * - list: 記事の一覧。変数 `locale`、`limit`（件数）を取る
 * - byId: 1件の記事。変数 `id`、`locale` を取る。include で返す項目を足す
 *   （bodyHtml・bodyMarkdown は、本文のフィールド `body` を変換器でHTML・Markdownにしたもの。M2）
 * - deep: 参照フィールド `related` を depth 段たどる（深さの検査用）
 * - wide: 一覧を aliases 個の別名で並べる（複雑さの検査用）
 * - redirects: ワークスペースのリダイレクト一覧（M2。読むのは driver.redirects.parse）
 * - correctionFeed: 訂正フィード（M2）。変数 `locale`、`limit` を取る
 * - byId の include の corrections・deletedAnchors・freshness は、記事の訂正履歴・削除済みアンカー・鮮度（M2）
 * - bySlug: スラッグで記事を1件。変数 `slug`、`locale` を取る。fields・include は byId と同じ
 * - byId の include の aiMarkdown は、記事のAI向けのMarkdown版（M2。素材にならない記事では空）
 * - aiCatalog: AI向けの記事の目録（M2）。変数 `locale`、`limit` を取る
 * - aiMarkdownByUrl: 公開URLで記事のMarkdown版（M2。連携部品の `.md` 用）。変数 `url` を取る
 * - text: GraphQLの文をそのまま使う
 */
export type QuerySpec =
  | { kind: "list"; fields?: string[] }
  | {
      kind: "byId";
      fields?: string[];
      include?: ("body" | "bodyHtml" | "bodyMarkdown" | "alternates" | "related" | "corrections" | "deletedAnchors" | "freshness" | "aiMarkdown")[];
    }
  | {
      kind: "bySlug";
      fields?: string[];
      include?: ("body" | "bodyHtml" | "bodyMarkdown" | "alternates" | "related" | "corrections" | "deletedAnchors" | "freshness" | "aiMarkdown")[];
    }
  | { kind: "deep"; depth: number }
  | { kind: "wide"; aliases: number }
  | { kind: "redirects" }
  | { kind: "correctionFeed" }
  | { kind: "aiCatalog" }
  | { kind: "aiMarkdownByUrl" }
  /** サイト内検索（M3、08-search-and-related.md）。変数は text・locale・first。 */
  | { kind: "search" }
  /** サイト内検索で、検索の方法（mode）も指定するもの（M3）。変数は text・locale・first・mode。 */
  | { kind: "searchWithMode" }
  | { kind: "text"; text: string };

/** サイト内検索の1件（1つの記事から、いちばん似たチャンクを1件）。 */
export interface SearchHit {
  entryId: string;
  locale: string;
  /** そのチャンクのセクションのアンカーID（見出しがなければ null）。 */
  anchor: string | null;
  /** 「タイトル ＞ 大見出し ＞ 小見出し」。 */
  breadcrumb: string[];
  /** 抜粋（プレーンテキスト、200文字まで）。 */
  excerpt: string;
  /** 類似度。 */
  score: number;
}

/** サイト内検索の応答。 */
export interface SearchResult {
  hits: SearchHit[];
  /** ベクトル検索が使えるか（予算切れ・LLMの接続先がない、なら使えない）。 */
  vectorSearch: "available" | "budgetExhausted" | "disabled";
  /** キーワード検索が使えるか（日本語の拡張がなければ disabled）。 */
  keywordSearch: "available" | "disabled";
}

/** サイト内検索の方法。hybrid はベクトル検索とキーワード検索の結果を混ぜる（既定）。 */
export type SearchMode = "hybrid" | "vector" | "keyword";

/** 登録済みクエリの参照。 */
export interface RegisteredQuery {
  workspaceId: string;
  /** 配信APIのリクエストで指定する値（ハッシュなど）。 */
  ref: string;
}

// ---------------------------------------------------------------------------
// Webhook・メディア・監査・メール
// ---------------------------------------------------------------------------

export type WebhookEventKind =
  | "published"
  | "updated"
  | "unpublished"
  | "deleted"
  | "corrected"
  | "erased"
  | "settingsChanged"
  | "urlChanged"
  | "scheduled"
  | "scheduleCancelled";

export interface ParsedWebhook {
  /** イベントの種類（ドライバーの webhookEventTypes の値のどれか）。 */
  type: string;
  workspaceId: string;
  entryId: string;
}

/** Webhookの署名の検証に必要なもの（署名の方式は手順書で公開する）。 */
export interface WebhookSigning {
  /** 署名に含まれる送信時刻（UNIX秒）。 */
  timestamp: number;
  /** 署名に含まれるイベントID。 */
  eventId: string;
  /** 署名の対象の文字列（時刻・イベントID・本文を結んだもの）。 */
  signingInput: string;
  /** ヘッダーに載っている署名（秘密の入れ替え中は複数）。 */
  signatures: string[];
  algorithm: "hmac-sha256";
  encoding: "hex" | "base64";
  /** 署名用の秘密を、HMACの鍵のバイト列に直したもの。 */
  keyBytes(secret: string): Buffer;
}

export interface UploadFile {
  filename: string;
  bytes: Buffer;
  contentType: string;
}

export interface AuditEntry {
  id: string;
  /** 操作の種類（ドライバーの auditActions の値のどれか）。 */
  action: string;
  actorUserId: string | null;
  /** 対象のID（記事、利用者、キーなど）。 */
  targetIds: string[];
  /** 記録の全体（そのままの形）。 */
  raw: unknown;
}

/** 監査ログの絞り込み（M2）。action はドライバーの auditActions の値。 */
export interface AuditFilter {
  action?: string;
  actorUserId?: string;
  from?: Date;
  to?: Date;
}

export type AuditActionKind =
  | "login"
  | "roleChanged"
  | "apiKeyIssued"
  | "apiKeyRevoked"
  | "personalTokenIssued"
  | "personalTokenRevoked"
  | "correctionPublished"
  | "versionErased"
  | "passwordResetIssued"
  | "mfaResetIssued"
  | "loginFailuresAlert"
  | "schemaDeployed";

export interface MailMessage {
  id: string;
  to: string[];
  subject: string;
  /** 本文（テキストとHTMLを結んだもの）。 */
  text: string;
  /** ヘッダーを含む生のメール。 */
  raw: string;
}

// ---------------------------------------------------------------------------
// CMSへの接続
// ---------------------------------------------------------------------------

export interface CmsClient {
  readonly baseUrl: string;

  // --- ログインとセッション ---
  login(creds: Credentials, options?: LoginOptions): Promise<Session>;
  /** ログインの全段階（パスワード、多要素認証）を行い、最後の応答をそのまま返す。 */
  loginRaw(creds: Credentials, options?: LoginOptions): Promise<HttpResponse>;
  /** パスワードの段階だけを行い、多要素認証のコードを送らない。その時点のCookieを返す。 */
  loginPasswordOnly(creds: Credentials, options?: LoginOptions): Promise<Session>;
  logout(session: Session): Promise<void>;
  /** 再認証。多要素認証を設定した人は多要素認証で、していない人はパスワードで行う。 */
  reauthenticate(session: Session, creds: Credentials): Promise<void>;
  /** 指定した方法だけで再認証を試み、応答をそのまま返す。 */
  reauthenticateRaw(session: Session, method: { password?: string; totpCode?: string }): Promise<HttpResponse>;
  /**
   * まだ使っていない時間の区切りのTOTPのコード（サーバーは同じ区切りのコードを二度受け付けない）。
   * ログインや再認証で使った区切りは使わず、なければ次の区切りまで待つ。
   */
  nextTotpCode(secret: string): Promise<string>;
  meRaw(auth: Auth, extras?: RequestExtras): Promise<HttpResponse>;
  me(session: Session): Promise<{ userId: string; email: string }>;
  changePassword(session: Session, creds: Credentials, newPassword: string): Promise<void>;
  changePasswordRaw(session: Session, currentPassword: string, newPassword: string): Promise<HttpResponse>;
  /** TOTPを設定し、秘密を返す（必要な再認証も行う）。 */
  enrollTotp(session: Session, creds: Credentials): Promise<string>;
  enrollPasskey(session: Session, creds: Credentials): Promise<PasskeyHandle>;
  reauthenticateWithPasskey(session: Session, passkey: PasskeyHandle): Promise<void>;
  /** ログイン中の本人の操作で、擬似IdPの次のログインの身元をひも付ける。 */
  linkOidcIdentity(session: Session): Promise<void>;
  /** 擬似IdP（support/oidc-idp.ts）でSSOログインする。 */
  loginWithOidcRaw(options?: LoginOptions): Promise<{ response: HttpResponse; session: Session }>;
  /** 本人のログインの履歴（応答をそのまま返す）。 */
  loginHistoryRaw(session: Session): Promise<HttpResponse>;

  // --- ワークスペースと所属 ---
  createWorkspace(session: Session, name: string, options?: { template?: string }): Promise<{ id: string }>;
  createWorkspaceRaw(auth: Auth, name: string): Promise<HttpResponse>;
  /**
   * 招待から承諾までを済ませ、新しい利用者を作る。
   * mfa を省略すると、admin のときだけ多要素認証（TOTP）を設定する。
   */
  createMember(adminSession: Session, workspaceId: string, role: WorkspaceRole, options?: { mfa?: boolean; email?: string }): Promise<Member>;
  /** 既存の利用者を招待し、本人の承諾まで済ませる。 */
  addExistingMember(adminSession: Session, workspaceId: string, member: Member, role: WorkspaceRole): Promise<void>;
  inviteRaw(adminSession: Session, workspaceId: string, email: string, role: WorkspaceRole): Promise<HttpResponse>;
  /** 招待を承諾する。新しい利用者はパスワードを決め、既存の利用者はログイン中のセッションで承諾する。 */
  acceptInviteRaw(token: string, as: { newPassword: string } | { session: Session }): Promise<HttpResponse>;
  changeMemberRole(session: Session, workspaceId: string, userId: string, role: WorkspaceRole): Promise<void>;
  /** 所属者が実際に持つ重い権限（ロールの既定と、個別に付与したもの）を読む。 */
  memberPermissions(session: Session, workspaceId: string, userId: string): Promise<HeavyPermission[]>;
  /** 所属者に個別に付与した重い権限（ロールの既定は含まない）を読む。 */
  memberGrants(session: Session, workspaceId: string, userId: string): Promise<HeavyPermission[]>;
  /** 所属者に個別に付与する重い権限を、渡したものに置き換える（応答をそのまま返す）。 */
  setMemberGrantsRaw(auth: Auth, workspaceId: string, userId: string, grants: HeavyPermission[]): Promise<HttpResponse>;
  /** 記事の公開設定のスイッチを変える（送った項目だけ。応答をそのまま返す）。 */
  setPublishSettingsRaw(
    auth: Auth,
    workspaceId: string,
    entryId: string,
    changes: { includeInAiMaterials?: boolean; optOutOfAiTraining?: boolean; useForChatbot?: boolean },
  ): Promise<HttpResponse>;
  /** 「確認済み」を記録する（応答をそのまま返す）。 */
  markVerifiedRaw(auth: Auth, workspaceId: string, entryId: string, memo: string): Promise<HttpResponse>;
  removeMember(session: Session, workspaceId: string, userId: string): Promise<void>;
  /** サーバー管理者を新しく作る（サーバー上のCLIで）。 */
  createServerAdmin(options: { mfa: boolean }): Promise<Member>;
  /**
   * サーバー管理者が、ワークスペースの上限を決める。埋め込みの予算は、ワークスペースが決める上限の天井（M3、Q70）。
   * null は設定ファイルの既定値に戻す。
   */
  setWorkspaceLimits(serverAdminSession: Session, workspaceId: string, limits: WorkspaceLimits): Promise<void>;
  /**
   * ワークスペースの埋め込みの作成の予算（トークン数、UTCの暦の月・日。null は上限なし（天井の範囲で））を決める（応答をそのまま返す）。
   * 重い権限（manageLlm）の操作で、再認証が要る。
   */
  setEmbeddingBudgetRaw(auth: Auth, workspaceId: string, budget: EmbeddingBudget): Promise<HttpResponse>;
  /** 埋め込みモデルの切り替えの見積もりと進み具合（サーバー管理者）。成功しなければ失敗する。 */
  embeddingSwitch(serverAdminSession: Session): Promise<EmbeddingSwitchStatus>;
  /**
   * 埋め込みモデルの切り替えを始める（止まっていれば再開する）。サーバー管理者の操作で、再認証が要る（応答をそのまま返す）。
   */
  startEmbeddingSwitchRaw(auth: Auth): Promise<HttpResponse>;

  // --- パスワードと多要素認証の回復 ---
  requestPasswordResetRaw(email: string, extras?: RequestExtras): Promise<HttpResponse>;
  completePasswordResetRaw(token: string, newPassword: string): Promise<HttpResponse>;
  /** 管理者が利用者のリセットを発行する。workspaceId が null ならインスタンスの操作として行う。 */
  issuePasswordResetRaw(session: Session, workspaceId: string | null, userId: string): Promise<HttpResponse>;
  issueMfaResetRaw(session: Session, workspaceId: string | null, userId: string): Promise<HttpResponse>;

  // --- キーとトークン ---
  createApiKey(session: Session, workspaceId: string, scopes: ApiKeyScope[], options?: { expiresAt?: Date }): Promise<IssuedSecret>;
  revokeApiKey(session: Session, workspaceId: string, keyId: string): Promise<void>;
  createPersonalToken(session: Session, workspaceIds: string[], options?: { scopes?: TokenScope[]; expiresAt?: Date }): Promise<IssuedSecret>;
  revokePersonalToken(session: Session, tokenId: string): Promise<void>;

  // --- 記事 ---
  createEntry(session: Session, workspaceId: string, input: EntryInput): Promise<{ id: string }>;
  createEntryRaw(auth: Auth, workspaceId: string, input: EntryInput): Promise<HttpResponse>;
  getDraft(session: Session, workspaceId: string, entryId: string): Promise<Draft>;
  listEntries(session: Session, workspaceId: string): Promise<{ id: string; slug: string; locale: string }[]>;
  /** 下書きを保存する。revision には保存の前に読んだリビジョン番号を渡す。 */
  /** 下書きを保存する。lockId は、編集のロック（M2）を持って保存するときに付ける。 */
  saveDraftRaw(auth: Auth, workspaceId: string, entryId: string, changes: Partial<EntryInput>, revision: string, options?: { lockId?: string }): Promise<HttpResponse>;
  /**
   * 記事の編集のロック（M2、Q73）。記事の画面を開いたときに取り、保存に lockId を付ける。ほかの画面が持っていれば断られる。
   * ロックは画面（ロックのID）ごと。保存はロックを取らない。
   */
  acquireEditLockRaw(auth: Auth, workspaceId: string, entryId: string): Promise<HttpResponse>;
  acquireEditLock(session: Session, workspaceId: string, entryId: string): Promise<string>;
  /** 編集を引き継ぐ（ほかの画面のロックを奪う）。新しいロックのIDを返す。 */
  takeOverEditLockRaw(auth: Auth, workspaceId: string, entryId: string): Promise<HttpResponse>;
  takeOverEditLock(session: Session, workspaceId: string, entryId: string): Promise<string>;
  /** ロックを外す（画面を離れたとき）。 */
  releaseEditLock(session: Session, workspaceId: string, entryId: string, lockId: string): Promise<void>;
  /** 最新のリビジョン番号を読んでから保存する。 */
  saveDraft(session: Session, workspaceId: string, entryId: string, changes: Partial<EntryInput>): Promise<void>;
  /** 公開する。note は「訂正として公開」のときの説明（自由記述）。 */
  publishEntry(session: Session, workspaceId: string, entryId: string, options?: { correction?: boolean; note?: string }): Promise<void>;
  publishRaw(auth: Auth, workspaceId: string, entryId: string, options?: { correction?: boolean; note?: string }): Promise<HttpResponse>;
  /**
   * いまの下書きを、指定した時刻に公開するよう予約する（M2、Q78）。予約し直すと前の予約を置き換える。
   * correction は「訂正として公開」の予約を試みるためのもの（予約できない: INV-SCHED-02）。
   */
  scheduleRaw(auth: Auth, workspaceId: string, entryId: string, at: Date, options?: { correction?: boolean; note?: string }): Promise<HttpResponse>;
  schedulePublish(session: Session, workspaceId: string, entryId: string, at: Date): Promise<void>;
  /** 予約を取り消す。 */
  cancelSchedule(session: Session, workspaceId: string, entryId: string): Promise<void>;
  /** 訂正の説明を書き直す（公開した後でも書ける。編集の履歴は管理画面の中だけに残る）。 */
  editCorrectionNote(session: Session, workspaceId: string, entryId: string, versionId: string, note: string): Promise<void>;
  /** 「確認済み」を記録する（最終確認日を進める。版は作らない）。 */
  markVerified(session: Session, workspaceId: string, entryId: string, memo: string): Promise<void>;
  unpublishEntry(session: Session, workspaceId: string, entryId: string): Promise<void>;
  deleteEntry(session: Session, workspaceId: string, entryId: string): Promise<void>;
  /** 記事の公開設定（外部AI向けの素材に含めるか、など）を1つ切り替える。 */
  changePublishSettings(session: Session, workspaceId: string, entryId: string): Promise<void>;
  /** 記事の「外部AI向けの素材に含める」を指定した値にする。 */
  setIncludeInAiMaterials(session: Session, workspaceId: string, entryId: string, include: boolean): Promise<void>;
  getEntryRaw(auth: Auth, workspaceId: string, entryId: string): Promise<HttpResponse>;
  listVersions(session: Session, workspaceId: string, entryId: string): Promise<VersionInfo[]>;
  getVersionRaw(auth: Auth, workspaceId: string, entryId: string, versionId: string): Promise<HttpResponse>;
  /** 抹消の画面に出す、削除の候補のメディアのID。 */
  eraseCandidates(session: Session, workspaceId: string, entryId: string, versionId: string): Promise<string[]>;
  /** 版の本文を抹消する（必要な再認証は呼び出し側で済ませる）。 */
  eraseVersionRaw(session: Session, workspaceId: string, entryId: string, versionId: string, deleteMediaIds: string[]): Promise<HttpResponse>;
  createTranslation(session: Session, workspaceId: string, sourceEntryId: string, locale: string): Promise<{ id: string }>;
  getTranslationInfo(session: Session, workspaceId: string, entryId: string): Promise<TranslationInfo>;
  /** 元の記事の、反映済みの版から今の版までの差分（M2）。 */
  getSourceDiff(session: Session, workspaceId: string, entryId: string): Promise<SourceDiff>;
  /** 「原文の更新を反映済み」にする（反映済みの版を、元の記事の今の版に進める。M2）。 */
  markSourceReflectedRaw(auth: Auth, workspaceId: string, entryId: string): Promise<HttpResponse>;
  markSourceReflected(session: Session, workspaceId: string, entryId: string): Promise<void>;
  /** レビューを依頼する（レビューする人は指定しない。M2）。 */
  requestReview(session: Session, workspaceId: string, entryId: string): Promise<void>;
  /** 「やること」の一覧（M2）。 */
  getTodo(session: Session, workspaceId: string): Promise<TodoItem[]>;
  getUrlHistory(session: Session, workspaceId: string, entryId: string): Promise<UrlChange[]>;

  // --- メディア ---
  uploadMediaRaw(session: Session, workspaceId: string, file: UploadFile): Promise<HttpResponse>;
  uploadMedia(session: Session, workspaceId: string, file: UploadFile): Promise<{ id: string; url: string }>;
  fetchMediaRaw(url: string): Promise<HttpResponse & { bytes: Buffer }>;

  // --- Webhook ---
  createWebhook(session: Session, workspaceId: string, url: string): Promise<{ id: string; secret: string }>;
  testWebhookRaw(session: Session, workspaceId: string, webhookId: string): Promise<HttpResponse>;

  // --- 配信API ---
  registerQueryRaw(workspaceIdInUrl: string, apiKey: string | null, spec: QuerySpec): Promise<HttpResponse>;
  registerQuery(workspaceId: string, registerKey: string, spec: QuerySpec): Promise<RegisteredQuery>;
  /**
   * 配信APIで登録済みのクエリを実行する。
   * @param workspaceIdInUrl URLに入れるワークスペースID
   * @param apiKey Authorization ヘッダーで送るキー。null なら送らない
   */
  deliveryRaw(
    workspaceIdInUrl: string,
    apiKey: string | null,
    query: RegisteredQuery,
    variables?: Record<string, unknown>,
    extras?: RequestExtras,
  ): Promise<HttpResponse>;
  /**
   * 登録済みのサイト内検索のクエリ（`{ kind: "search" }` または `{ kind: "searchWithMode" }`）を実行し、応答を読む。
   * 成功しなければ失敗する。`mode` は `searchWithMode` のクエリにだけ渡す。
   */
  search(workspaceId: string, apiKey: string, query: RegisteredQuery, input: { text: string; locale?: string; first?: number; mode?: SearchMode }): Promise<SearchResult>;
  /** 登録していないクエリを、文のまま送る。 */
  deliveryAdHocRaw(workspaceIdInUrl: string, apiKey: string, spec: QuerySpec, variables?: Record<string, unknown>): Promise<HttpResponse>;
  /** 1回のリクエストで複数のクエリを送る（バッチ）。 */
  deliveryBatchRaw(workspaceIdInUrl: string, apiKey: string, queries: RegisteredQuery[]): Promise<HttpResponse>;
  /** 配信APIで登録済みのクエリを、プレビュー用トークンを付けて実行する（M2）。apiKey が null ならキーは送らない。 */
  deliveryPreviewRaw(
    workspaceIdInUrl: string,
    apiKey: string | null,
    previewToken: string,
    query: RegisteredQuery,
    variables?: Record<string, unknown>,
  ): Promise<HttpResponse>;

  // --- プレビュー（M2） ---
  /** 記事のプレビュー用トークンを発行する。ttlSeconds を省略すると既定の有効期間。 */
  issuePreviewToken(session: Session, workspaceId: string, entryId: string, options?: { ttlSeconds?: number }): Promise<{ id: string; token: string }>;
  /** プレビュー用トークンを失効させる。 */
  revokePreviewToken(session: Session, workspaceId: string, entryId: string, tokenId: string): Promise<void>;

  // --- スキーマ ---
  deployTemplateRaw(auth: Auth, template: TemplateSpec, options?: { allowDestructive?: boolean; allowBrokenQueries?: boolean }): Promise<HttpResponse>;
  /** デプロイ用のトークン（サーバー管理者・デプロイの範囲・必要な再認証を済ませたもの）。 */
  deployToken(serverAdminSession: Session, creds: Credentials): Promise<string>;
  schemaInfoRaw(auth: Auth, workspaceId: string): Promise<HttpResponse>;
  migrationState(session: Session, workspaceId: string): Promise<"ok" | "pending">;

  // --- 監査ログ ---
  /** 監査ログを読む。workspaceId が null ならインスタンス全体のログ。 */
  readAuditLog(session: Session, workspaceId: string | null): Promise<AuditEntry[]>;
  /** 監査ログの1ページ（workspaceId が null ならインスタンス全体）。filter は絞り込み（M2）。 */
  auditLogRaw(auth: Auth, workspaceId: string | null, filter?: AuditFilter): Promise<HttpResponse>;
  /** 監査ログのダウンロード（1行1件の JSON。M2）。 */
  exportAuditLogRaw(auth: Auth, workspaceId: string | null, filter?: AuditFilter): Promise<HttpResponse>;
  /** ワークスペースの監査ログの書き出し先（Webhook）を作る（M2）。 */
  createAuditSink(session: Session, workspaceId: string, url: string): Promise<{ id: string; secret: string }>;
  createAuditSinkRaw(auth: Auth, workspaceId: string, url: string): Promise<HttpResponse>;

  // --- その他 ---
  /** 管理画面（SPA）の配信。 */
  adminUiRaw(path: string): Promise<HttpResponse>;
  /** 別のドメインを Origin にしたリクエスト。 */
  corsProbeRaw(kind: "adminApi" | "deliveryApi", method: "GET" | "OPTIONS", origin: string, workspaceId: string): Promise<HttpResponse>;

  /** 管理APIの、ログインが必要なすべての操作の一覧。 */
  readonly adminOperations: AdminOperation[];
}

// ---------------------------------------------------------------------------
// サーバーの起動・CLI
// ---------------------------------------------------------------------------

/** サーバーの設定を、実装の書式によらない形で表したもの。 */
export interface ServerConfig {
  /** 本番の設定として起動するか。 */
  production: boolean;
  /** 開発モード（任意のクエリを受け付ける。Q115）。 */
  devMode: boolean;
  /** 秘密の値（設定の名前 → 値）。名前の一覧は secretSettingNames。 */
  secrets: Record<string, string>;
  /** 暗号化の鍵のファイル。null なら設定しない。 */
  encryptionKeyFile: string | null;
  /** 普段の処理用のDBの接続先。 */
  databaseUrl: string;
  /** 移行・ロールの作成用のDBの接続先（Q173）。 */
  migrationDatabaseUrl: string;
  /** 公開するURL。本番では https であることが前提（Q168）。 */
  publicUrl: string;
  /** 外部への送信の例外として許す内部の宛先（ホスト名かIPアドレス）。HTTPも許す（Q139）。 */
  internalDestinationExceptions: string[];
  /** セッションの有効期限（秒）。 */
  sessionLifetimeSeconds?: number;
  /** 再認証が有効な時間（秒）。 */
  reauthWindowSeconds?: number;
  /** リセット・招待のトークンの有効期限（秒）。 */
  oneTimeTokenLifetimeSeconds?: number;
  /** 回数制限（1分あたり）。 */
  rateLimits?: { deliveryPerKey?: number; deliveryPerWorkspace?: number };
  /**
   * 接続元のIPアドレスごとのログインの試行の上限（1分あたり）。undefined ならサーバーの既定。
   * テストの舞台のCMSでは、接続元を名乗らないログイン（舞台の準備など）が同じアドレスに集まるので大きくする。
   */
  loginAttemptsPerIpPerMinute?: number;
  /** 共有キャッシュを許すか（Q125）。 */
  sharedCache?: boolean;
  /** 出口のプロキシ（Q128）。 */
  egressProxy?: { url: string };
  /**
   * LLMの接続先（インスタンスの既定値。M3は埋め込みだけ）。なければ埋め込みを作らない。
   * テストの舞台のCMSは、擬似LLM（support/mock-llm.ts）に決まったキーでつなぐ。
   * 呼び出しを細かく確かめるテストは、専用のキーで別のサーバーを起動する（擬似LLMの記録はキーごと）。
   */
  llm?: {
    baseUrl: string;
    apiKey: string;
    embeddingModel: string;
    embeddingDimensions: number;
    pricePerMillionTokens?: number;
    /**
     * 切り替え先の埋め込みのモデル（`[llm.next_embedding]`）。接続先とキーは、省略すれば今のものを使う。
     * 接続先を書くときは、キーも書く。
     */
    nextEmbedding?: { model: string; dimensions: number; baseUrl?: string; apiKey?: string; pricePerMillionTokens?: number };
    /** ワークスペースの埋め込みの予算の天井の既定値（トークン数、月・日）。なければ 0。 */
    defaultWorkspaceTokensPerMonth?: number;
    defaultWorkspaceTokensPerDay?: number;
  };
  /** 漏えいしたパスワードの照会先（Have I Been Pwned の range API と同じ形）。null なら無効。 */
  breachedPasswordApi?: { baseUrl: string } | null;
  /**
   * 全員に多要素認証を必須にするか（Q197）。undefined ならサーバーの既定（有効）。
   * テスト環境の設定（validServerConfig）では、舞台に多要素認証のない所属者を作るため false にする。
   */
  mfaRequiredForEveryone?: boolean;
  /** ログインの履歴の保存期間（秒）と、期限切れを消す処理の間隔（秒）。 */
  loginHistoryRetention?: { retentionSeconds: number; sweepIntervalSeconds: number };
  /**
   * 組み込みJSの1回の実行あたりのメモリと実行時間の上限（Q89）。undefined ならサーバーの既定。
   * timeMs は同期（URL生成関数・同期のフック）、asyncTimeMs は非同期のフック。
   */
  scriptLimits?: { memoryBytes: number; timeMs: number; asyncTimeMs: number };
  /** ログの詳しさ。 */
  logLevel?: "error" | "warn" | "info" | "debug" | "trace";
  /** 管理者への通知の急増のしきい値（M2）。undefined ならサーバーの既定。 */
  alerts?: { loginFailures?: number; rateLimited?: number; windowSeconds?: number; cooldownSeconds?: number };
  /** インスタンス全体の監査ログの書き出し先（Webhook。M2）。secret はドライバーがファイルに書いて渡す。 */
  auditExportWebhooks?: { url: string; secret: string }[];
}

export interface StartedServer {
  /** "ready": 受け付けを始めた / "exited": 受け付けを始めずに終了した。 */
  outcome: "ready" | "exited";
  exitCode: number | null;
  /** 標準出力と標準エラー出力（停止するまでの分。stop の後に読むとすべて入っている）。 */
  readonly output: string;
  /** outcome が "ready" のときの接続先。 */
  client: CmsClient | null;
  stop(): Promise<void>;
}

export interface CliResult {
  exitCode: number;
  output: string;
}

/** 他のテストと共有しない、新しいインスタンス（新しいDBで初期化し、サーバー管理者を作ったもの）。 */
export interface IsolatedInstance {
  config: ServerConfig;
  serverAdmin: Member;
}

export interface ImportOptions {
  sourceDir: string;
  workspaceId: string;
  /** 取り込みに使う個人トークン。 */
  token: string;
  /** 独自部品とカスタムノードの対応（部品名 → ノード名）。 */
  componentMap?: Record<string, string>;
  /** 翻訳グループにまとめるfrontmatterのキー。 */
  translationKey?: string;
  /** 元の記事を指定するfrontmatterのキー。 */
  translationOf?: string;
  /** 元のURLを持つfrontmatterのキー。 */
  originalUrlKey: string;
  /** リダイレクト設定の書き出し先。 */
  redirectOutput: string;
}

// ---------------------------------------------------------------------------
// DBの権限のテスト用（INV-DB-* と INV-CFG-03 だけが使う）
// ---------------------------------------------------------------------------

export interface DbLayout {
  workspaceSchema(workspaceId: string): Promise<string>;
  workspaceRole(workspaceId: string): Promise<string>;
  sharedSchemas(): Promise<string[]>;
  auditLogTables(): Promise<string[]>;
}

// ---------------------------------------------------------------------------
// 公開する入口
// ---------------------------------------------------------------------------

export const driver = {
  /** 起動済みのCMSにつなぐ。 */
  connect(baseUrl: string): CmsClient {
    return connectAdmin(baseUrl, todo);
  },

  /** テスト環境のCMSと同じDBを使う、本番の設定として正しい設定（各テストはここから一部だけ変える）。 */
  async validServerConfig(): Promise<ServerConfig> {
    return validServerConfigFromEnv();
  },

  /** 設定の秘密の値の名前の一覧。 */
  secretSettingNames(): string[] {
    return [...SECRET_SETTING_NAMES];
  },

  /** 設定を書き出してサーバーを起動し、受け付けを始めるか終了するまで待つ。 */
  startServer(config: ServerConfig): Promise<StartedServer> {
    return startServerProcess(config, (baseUrl) => driver.connect(baseUrl));
  },

  /** 新しいDBで初期化（`cms-server init`）し、サーバー管理者を作る。 */
  createIsolatedInstance(): Promise<IsolatedInstance> {
    return createIsolatedInstance();
  },

  /** 緊急停止: すべてのセッションと個人トークンを失効させる（サーバー上のCLI）。 */
  emergencyRevokeAllSessions(config: ServerConfig): Promise<CliResult> {
    return runServerCli(config, ["revoke-all-sessions", "--yes"]);
  },

  /** 暗号化の鍵を入れ替える（サーバー上のCLIで新しい鍵を加えて再暗号化し、古い鍵を外した設定を返す）。 */
  rotateEncryptionKey(config: ServerConfig): Promise<{ config: ServerConfig; result: CliResult }> {
    return rotateEncryptionKeyWithCli(config);
  },

  /** Markdownの取り込みCLI（Node.js）を実行する。 */
  runImport(cmsUrl: string, options: ImportOptions): Promise<CliResult> {
    return runImportCli(cmsUrl, options);
  },

  /** 取り込みCLIが書き出したリダイレクト設定を読む（Nuxt の routeRules。cli/src/import/run.ts）。 */
  async readRedirectFile(path: string): Promise<{ from: string; to: string }[]> {
    const parsed = JSON.parse(readFileSync(path, "utf8")) as { routeRules?: Record<string, { redirect?: { to?: unknown } }> };
    if (typeof parsed.routeRules !== "object" || parsed.routeRules === null) throw new Error(`リダイレクト設定に routeRules がない: ${path}`);
    return Object.entries(parsed.routeRules).map(([from, rule]) => {
      if (typeof rule.redirect?.to !== "string") throw new Error(`リダイレクト設定の ${from} に redirect.to がない: ${path}`);
      return { from, to: rule.redirect.to };
    });
  },

  /** 公式のDocker Composeファイルのパス。 */
  officialComposeFile(): string {
    return fileURLToPath(new URL("../../../deploy/docker-compose.yml", import.meta.url));
  },

  /** ライブプレビュー（M2）。 */
  livePreview: {
    /**
     * ライブプレビューのメッセージの受け手（packages/preview の acceptPreviewMessage）に、正しい形のメッセージを1つ渡し、
     * 受け付けたかを返す。origin は送り元のオリジン、fromExpectedWindow は送り元が期待した窓（iframe や親の窓）か。
     * allowedOrigins は受け手に設定したオリジンの一覧。
     */
    async accepts(message: { origin: string; fromExpectedWindow: boolean }, allowedOrigins: string[]): Promise<boolean> {
      const mod = (await import(new URL("../../../packages/preview/dist/index.js", import.meta.url).href)) as {
        acceptPreviewMessage: (event: { origin: string; source: unknown; data: unknown }, expected: { origins: string[]; source: unknown }) => unknown;
      };
      const expectedWindow = {};
      const accepted = mod.acceptPreviewMessage(
        { origin: message.origin, source: message.fromExpectedWindow ? expectedWindow : {}, data: { type: "cms:preview:ready" } },
        { origins: allowedOrigins, source: expectedWindow },
      );
      return accepted !== null;
    },
  },

  /** 本文のCMS固有のノード。 */
  body: {
    // CMSの画像のノード（属性は server/src/content/body.rs）
    image: (media: { mediaId: string; alt?: string; decorative?: boolean }): TiptapNode => ({
      type: "image",
      attrs: Object.fromEntries(Object.entries({ mediaId: media.mediaId, alt: media.alt, decorative: media.decorative }).filter(([, v]) => v !== undefined)),
    }),
    embed: (url: string): TiptapNode => ({ type: "embed", attrs: { src: url } }),
    /** 見出しにアンカーIDを付けたもの（アンカーIDは見出しの属性 `id`）。 */
    withAnchor: (headingNode: TiptapNode, anchorId: string): TiptapNode => ({ ...headingNode, attrs: { ...headingNode.attrs, id: anchorId } }),
    anchorIdOf: (headingNode: TiptapNode): string | undefined => {
      const id = headingNode.attrs?.["id"];
      return typeof id === "string" ? id : undefined;
    },
  },

  /** リダイレクト一覧（配信APIの redirects）の読み取り。 */
  redirects: {
    /** 応答の本文を、{from, to, status} の一覧にする（to は 410 のとき null）。形が違えば例外を投げる。 */
    parse: (body: string): RedirectRule[] => {
      const v = JSON.parse(body) as { data?: { redirects?: unknown } };
      const list = v.data?.redirects;
      if (!Array.isArray(list)) throw new Error(`リダイレクト一覧の応答ではない: ${body.slice(0, 200)}`);
      return list.map((r: { from?: unknown; to?: unknown; status?: unknown }) => {
        if (typeof r.from !== "string" || (r.to !== null && typeof r.to !== "string") || typeof r.status !== "number") {
          throw new Error(`リダイレクト一覧の項目の形が違う: ${JSON.stringify(r)}`);
        }
        return { from: r.from, to: r.to as string | null, status: r.status };
      });
    },
  },

  /** Webhookの読み取り。 */
  // 種類の名前・本文・署名の書式は server/src/webhooks.rs に合わせる
  webhook: {
    eventTypes: (): Record<WebhookEventKind, string> => ({
      published: "entry.published",
      updated: "entry.updated",
      unpublished: "entry.unpublished",
      deleted: "entry.deleted",
      corrected: "entry.corrected",
      erased: "entry.version_erased",
      settingsChanged: "entry.settings_changed",
      urlChanged: "entry.url_changed",
      scheduled: "entry.scheduled",
      scheduleCancelled: "entry.schedule_cancelled",
    }),
    parse: (body: string): ParsedWebhook => {
      const v = JSON.parse(body) as { type?: unknown; workspaceId?: unknown; entryId?: unknown };
      if (typeof v.type !== "string" || typeof v.workspaceId !== "string" || typeof v.entryId !== "string") {
        throw new Error(`記事のイベントのWebhookの本文ではない: ${body.slice(0, 200)}`);
      }
      return { type: v.type, workspaceId: v.workspaceId, entryId: v.entryId };
    },
    // ヘッダー: `Webhook-Signature: t=<UNIX秒>,id=<イベントID>,v1=<16進>[,v1=<16進>]`。署名の対象は `<t>.<id>.<本文>`
    signing: (headers: Record<string, string>, body: string): WebhookSigning => {
      const raw = headers["webhook-signature"];
      if (raw === undefined) throw new Error("Webhookに署名のヘッダー（webhook-signature）がない");
      const parts = raw.split(",").map((p) => {
        const at = p.indexOf("=");
        return at < 0 ? [p.trim(), ""] : [p.slice(0, at).trim(), p.slice(at + 1).trim()];
      });
      const one = (key: string): string => {
        const found = parts.filter(([k]) => k === key);
        if (found.length !== 1) throw new Error(`署名のヘッダーの ${key} が1つではない: ${raw}`);
        return found[0]![1]!;
      };
      const t = one("t");
      if (!/^[0-9]+$/.test(t)) throw new Error(`署名のヘッダーの t が数ではない: ${raw}`);
      const eventId = one("id");
      return {
        timestamp: Number(t),
        eventId,
        signingInput: `${t}.${eventId}.${body}`,
        signatures: parts.filter(([k]) => k === "v1").map(([, v]) => v!),
        algorithm: "hmac-sha256",
        encoding: "hex",
        // HMACの鍵は、秘密の文字列（whsec_...）のバイト列そのもの
        keyBytes: (secret: string) => Buffer.from(secret, "utf8"),
      };
    },
  },

  /** 監査ログの操作の種類の名前。 */
  // 名前は server/src/audit.rs の action に合わせる
  auditActions: (): Record<AuditActionKind, string> => ({
    login: "auth.login",
    roleChanged: "member.role_changed",
    apiKeyIssued: "api_key.issued",
    apiKeyRevoked: "api_key.revoked",
    personalTokenIssued: "personal_token.issued",
    personalTokenRevoked: "personal_token.revoked",
    correctionPublished: "entry.published_as_correction",
    versionErased: "version.erased",
    passwordResetIssued: "password_reset.issued",
    mfaResetIssued: "mfa_reset.issued",
    loginFailuresAlert: "alert.login_failures",
    schemaDeployed: "template.deployed",
  }),

  /** テスト環境のメールの受け口（SMTPの受け手）。 */
  mail: {
    // テスト環境の準備（support/global-setup.ts）が起動したSMTPの受け手のメールを読む
    list: async (to: string): Promise<MailMessage[]> => {
      const dir = process.env["CMS_TEST_MAIL_DIR"];
      if (!dir) return todo("mail.list（CMS_TEST_MAIL_DIR がない）");
      return listMail(dir, to);
    },
    /** リセット・招待のメールから、トークンを取り出す。 */
    tokenFrom: (message: MailMessage, kind: "reset" | "invite"): string | null => {
      // リンクの形は server/src/invitations.rs と server/src/resets.rs の link() に合わせる
      const path = kind === "invite" ? "/admin/invitations/accept" : "/admin/reset-password";
      const at = message.text.indexOf(`${path}#token=`);
      if (at < 0) return null;
      return /^[A-Za-z0-9_-]+/.exec(message.text.slice(at + path.length + "#token=".length))?.[0] ?? null;
    },
  },

  // ワークスペースのスキーマとロールの名前は、CMSが共通のスキーマ（cms.workspaces）に登録した値を読む
  db: {
    workspaceSchema: async (workspaceId: string): Promise<string> => (await registeredDbNames(workspaceId)).schema,
    workspaceRole: async (workspaceId: string): Promise<string> => (await registeredDbNames(workspaceId)).role,
    // 共通のスキーマ（server/src/migrate.rs の SHARED_SCHEMA）
    sharedSchemas: async (): Promise<string[]> => ["cms"],
    // 監査ログのテーブル（server/migrations/0001_init.sql）
    auditLogTables: async (): Promise<string[]> => ["cms.audit_log"],
  } satisfies DbLayout,
};

/**
 * 他のテストと共有しない、新しいインスタンス。テスト環境と同じロールで新しいDBを作り、
 * サーバーを一度起動して移行し、サーバー管理者（多要素認証あり）を作り、BASE_TEMPLATE をデプロイする。
 * 暗号化の鍵のファイルは写しを使う（鍵の入れ替えのテストが、テスト環境の鍵を変えないように）。
 */
async function createIsolatedInstance(): Promise<IsolatedInstance> {
  const base = validServerConfigFromEnv();
  const dbName = `cms_iso_${randomUUID().replace(/-/g, "").slice(0, 16)}`;
  const roleOf = (url: string) => decodeURIComponent(new URL(url).username);
  await withClient(env.dbSuperuserUrl, async (c) => {
    await c.query(`CREATE DATABASE ${ident(dbName)} OWNER ${ident(roleOf(base.migrationDatabaseUrl))}`);
    await c.query(`GRANT CONNECT ON DATABASE ${ident(dbName)} TO ${ident(roleOf(base.databaseUrl))}`);
  });
  // 検索用データのベクトルの拡張は、スーパーユーザーが前もって作る（scripts/test-env/setup.sh と同じ）
  const isolatedSuperuserUrl = new URL(env.dbSuperuserUrl);
  isolatedSuperuserUrl.pathname = `/${dbName}`;
  await withClient(isolatedSuperuserUrl.toString(), async (c) => {
    await c.query("CREATE EXTENSION IF NOT EXISTS vector");
  });
  const withDb = (url: string) => {
    const u = new URL(url);
    u.pathname = `/${dbName}`;
    return u.toString();
  };
  const keyDir = mkdtempSync(join(tmpdir(), "cms-iso-key-"));
  const keyFile = join(keyDir, "encryption.key");
  if (base.encryptionKeyFile === null) throw new Error("テスト環境の暗号化の鍵のファイルがない");
  copyFileSync(base.encryptionKeyFile, keyFile);
  const config: ServerConfig = {
    ...base,
    databaseUrl: withDb(base.databaseUrl),
    migrationDatabaseUrl: withDb(base.migrationDatabaseUrl),
    encryptionKeyFile: keyFile,
  };

  const started = await driver.startServer(config);
  try {
    if (started.outcome !== "ready" || !started.client) throw new Error(`新しいインスタンスのサーバーが起動しない:\n${started.output}`);
    const cms = started.client;
    const email = `inv-iso-root-${randomUUID().slice(0, 8)}@inv.test`;
    const created = await runServerCli(config, ["create-server-admin", "--email", email]);
    const password = created.output.match(/Password \(shown only once\): (\S+)/)?.[1];
    if (created.exitCode !== 0 || !password) throw new Error(`新しいインスタンスのサーバー管理者を作れない:\n${created.output}`);
    const first = await cms.loginPasswordOnly({ email, password });
    const totpSecret = await cms.enrollTotp(first, { email, password });
    const creds = { email, password, totpSecret };
    const session = await cms.login(creds);
    const { userId } = await cms.me(session);
    const token = await cms.deployToken(session, creds);
    const res = await cms.deployTemplateRaw(asBearer(token), BASE_TEMPLATE);
    if (res.status < 200 || res.status >= 300) throw new Error(`新しいインスタンスに BASE_TEMPLATE をデプロイできない（${res.status}）: ${res.text}`);
    return { config, serverAdmin: { ...creds, userId } };
  } finally {
    await started.stop();
  }
}

/**
 * 取り込みのCLI（cli/、ビルド済みの dist/index.js）を実行する。CMS_TEST_CLI_BIN で別の場所を指定できる。
 * 取り込みの設定はJSONのファイルに書いて渡し、トークンは環境変数 CMS_TOKEN で渡す（cli/src/index.ts）。
 */
async function runImportCli(cmsUrl: string, options: ImportOptions, timeoutMs = 120_000): Promise<CliResult> {
  const bin = process.env["CMS_TEST_CLI_BIN"] ?? fileURLToPath(new URL("../../../cli/dist/index.js", import.meta.url));
  const dir = mkdtempSync(join(tmpdir(), "cms-import-config-"));
  const configPath = join(dir, "import.json");
  const config: Record<string, unknown> = { originalUrlKey: options.originalUrlKey, componentMap: options.componentMap ?? {} };
  if (options.translationKey) config["translationKey"] = options.translationKey;
  if (options.translationOf) config["translationOf"] = options.translationOf;
  writeFileSync(configPath, JSON.stringify(config));
  try {
    const args = [bin, "import", options.sourceDir, "--url", cmsUrl, "--workspace", options.workspaceId, "--config", configPath, "--redirects", options.redirectOutput];
    const child = spawn(process.execPath, args, { stdio: ["ignore", "pipe", "pipe"], env: { ...process.env, CMS_TOKEN: options.token } });
    let output = "";
    child.stdout.on("data", (c: Buffer) => (output += c.toString("utf8")));
    child.stderr.on("data", (c: Buffer) => (output += c.toString("utf8")));
    const exitCode = await new Promise<number>((resolve, reject) => {
      const timer = setTimeout(() => {
        child.kill("SIGKILL");
        reject(new Error(`取り込みのCLIが ${timeoutMs}ms 以内に終わらなかった:\n${output}`));
      }, timeoutMs);
      child.on("error", reject);
      child.on("exit", (code) => {
        clearTimeout(timer);
        resolve(code ?? -1);
      });
    });
    return { exitCode, output };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

/** CMSが登録したワークスペースのDBの名前（スキーマとロール）。 */
async function registeredDbNames(workspaceId: string): Promise<{ schema: string; role: string }> {
  return withClient(env.dbSuperuserUrl, async (c) => {
    const r = await c.query<{ db_schema: string; db_role: string }>("SELECT db_schema, db_role FROM cms.workspaces WHERE id = $1", [workspaceId]);
    const row = r.rows[0];
    if (!row) throw new Error(`ワークスペース ${workspaceId} がDBに登録されていない`);
    return { schema: row.db_schema, role: row.db_role };
  });
}
