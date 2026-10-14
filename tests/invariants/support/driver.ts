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
import { copyFileSync, mkdtempSync } from "node:fs";
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
 * - deep: 参照フィールド `related` を depth 段たどる（深さの検査用）
 * - wide: 一覧を aliases 個の別名で並べる（複雑さの検査用）
 * - text: GraphQLの文をそのまま使う
 */
export type QuerySpec =
  | { kind: "list"; fields?: string[] }
  | { kind: "byId"; fields?: string[]; include?: ("body" | "alternates" | "related")[] }
  | { kind: "deep"; depth: number }
  | { kind: "wide"; aliases: number }
  | { kind: "text"; text: string };

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
  | "urlChanged";

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
  removeMember(session: Session, workspaceId: string, userId: string): Promise<void>;
  /** サーバー管理者を新しく作る（サーバー上のCLIで）。 */
  createServerAdmin(options: { mfa: boolean }): Promise<Member>;
  setWorkspaceLimits(serverAdminSession: Session, workspaceId: string, limits: { mediaBytes?: number }): Promise<void>;

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
  saveDraftRaw(auth: Auth, workspaceId: string, entryId: string, changes: Partial<EntryInput>, revision: string): Promise<HttpResponse>;
  /** 最新のリビジョン番号を読んでから保存する。 */
  saveDraft(session: Session, workspaceId: string, entryId: string, changes: Partial<EntryInput>): Promise<void>;
  publishEntry(session: Session, workspaceId: string, entryId: string, options?: { correction?: boolean }): Promise<void>;
  publishRaw(auth: Auth, workspaceId: string, entryId: string, options?: { correction?: boolean }): Promise<HttpResponse>;
  unpublishEntry(session: Session, workspaceId: string, entryId: string): Promise<void>;
  deleteEntry(session: Session, workspaceId: string, entryId: string): Promise<void>;
  /** 記事の公開設定（外部AI向けの素材に含めるか、など）を1つ切り替える。 */
  changePublishSettings(session: Session, workspaceId: string, entryId: string): Promise<void>;
  getEntryRaw(auth: Auth, workspaceId: string, entryId: string): Promise<HttpResponse>;
  listVersions(session: Session, workspaceId: string, entryId: string): Promise<VersionInfo[]>;
  getVersionRaw(auth: Auth, workspaceId: string, entryId: string, versionId: string): Promise<HttpResponse>;
  /** 抹消の画面に出す、削除の候補のメディアのID。 */
  eraseCandidates(session: Session, workspaceId: string, entryId: string, versionId: string): Promise<string[]>;
  /** 版の本文を抹消する（必要な再認証は呼び出し側で済ませる）。 */
  eraseVersionRaw(session: Session, workspaceId: string, entryId: string, versionId: string, deleteMediaIds: string[]): Promise<HttpResponse>;
  createTranslation(session: Session, workspaceId: string, sourceEntryId: string, locale: string): Promise<{ id: string }>;
  getTranslationInfo(session: Session, workspaceId: string, entryId: string): Promise<TranslationInfo>;
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
  /** 登録していないクエリを、文のまま送る。 */
  deliveryAdHocRaw(workspaceIdInUrl: string, apiKey: string, spec: QuerySpec, variables?: Record<string, unknown>): Promise<HttpResponse>;
  /** 1回のリクエストで複数のクエリを送る（バッチ）。 */
  deliveryBatchRaw(workspaceIdInUrl: string, apiKey: string, queries: RegisteredQuery[]): Promise<HttpResponse>;

  // --- スキーマ ---
  deployTemplateRaw(auth: Auth, template: TemplateSpec, options?: { allowDestructive?: boolean; allowBrokenQueries?: boolean }): Promise<HttpResponse>;
  /** デプロイ用のトークン（サーバー管理者・デプロイの範囲・必要な再認証を済ませたもの）。 */
  deployToken(serverAdminSession: Session, creds: Credentials): Promise<string>;
  schemaInfoRaw(auth: Auth, workspaceId: string): Promise<HttpResponse>;
  migrationState(session: Session, workspaceId: string): Promise<"ok" | "pending">;

  // --- 監査ログ ---
  /** 監査ログを読む。workspaceId が null ならインスタンス全体のログ。 */
  readAuditLog(session: Session, workspaceId: string | null): Promise<AuditEntry[]>;

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
  /** 漏えいしたパスワードの照会先（Have I Been Pwned の range API と同じ形）。null なら無効。 */
  breachedPasswordApi?: { baseUrl: string } | null;
  /**
   * 全員に多要素認証を必須にするか（Q197）。undefined ならサーバーの既定（有効）。
   * テスト環境の設定（validServerConfig）では、舞台に多要素認証のない所属者を作るため false にする。
   */
  mfaRequiredForEveryone?: boolean;
  /** ログインの履歴の保存期間（秒）と、期限切れを消す処理の間隔（秒）。 */
  loginHistoryRetention?: { retentionSeconds: number; sweepIntervalSeconds: number };
  /** ログの詳しさ。 */
  logLevel?: "error" | "warn" | "info" | "debug" | "trace";
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
  runImport(_cmsUrl: string, _options: ImportOptions): Promise<CliResult> {
    return todo("runImport");
  },

  /** 取り込みCLIが書き出したリダイレクト設定を読む。 */
  readRedirectFile(_path: string): Promise<{ from: string; to: string }[]> {
    return todo("readRedirectFile");
  },

  /** 公式のDocker Composeファイルのパス。 */
  officialComposeFile(): string {
    return todo("officialComposeFile");
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
  auditActions: (): Record<AuditActionKind, string> => todo("auditActions"),

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

/** CMSが登録したワークスペースのDBの名前（スキーマとロール）。 */
async function registeredDbNames(workspaceId: string): Promise<{ schema: string; role: string }> {
  return withClient(env.dbSuperuserUrl, async (c) => {
    const r = await c.query<{ db_schema: string; db_role: string }>("SELECT db_schema, db_role FROM cms.workspaces WHERE id = $1", [workspaceId]);
    const row = r.rows[0];
    if (!row) throw new Error(`ワークスペース ${workspaceId} がDBに登録されていない`);
    return { schema: row.db_schema, role: row.db_role };
  });
}
