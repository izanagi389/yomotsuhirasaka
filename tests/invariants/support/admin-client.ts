// ドライバーの一部: 管理APIへの接続（YomiClient）。管理APIのURLや本文の形は、ここに閉じ込める。
// 規則は driver.ts の冒頭のとおり（合否の判定を書かない、失敗を成功に見せかけない、Raw は応答をそのまま返す）。
//
// サーバーにまだない操作は、呼び出されたときに NotImplementedError を投げる。

import { createHash, randomUUID } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { AdminOperation, AnswerError, AnswerSettings, AssistBudget, AssistSettings, ConflictCheck, WorkspaceLlm, QuestionCandidate, QuestionLogSettings, AnswerEvent, AnswerResult, AnswerSource, AnswerStatus, ApiKeyScope, AuditEntry, AuditFilter, Auth, YomiClient, Credentials, Draft, EmbeddingBudget, EmbeddingSwitchStatus, EntryInput, HeavyPermission, QuerySpec, RegisteredQuery, IssuedSecret, SearchHit, SearchMode, SearchResult, RelatedEntry, RelatedSettings, LoginOptions, Member, OperationTarget, PasskeyHandle, RequestExtras, Session, TokenScope, SourceDiff, TemplateMapping, TodoItem, TranslationInfo, UploadFile, UrlChange, VersionInfo, WorkspaceLimits, WorkspaceRole } from "./driver.js";
import type { TiptapDoc } from "./body.js";
import { mailCursor, waitForToken } from "./mail.js";
import { isSuccess, parseSetCookie, toHttpResponse, type HttpResponse } from "./http.js";
import { totp } from "./totp.js";
import { createCredential, getAssertion, type SoftPasskey } from "./webauthn.js";
import { toManifest } from "./manifest.js";
import { runServerCli, validServerConfigFromEnv } from "./server-process.js";
import { BASE_TEMPLATE, cloneTemplate, postType, type TemplateSpec } from "./template.js";

/** セッションのCookieの名前以外の、管理APIの細部。 */
const ADMIN = "/api/admin";
const CSRF_HEADER = "X-CSRF-Token";

interface RequestOptions {
  auth?: Auth;
  body?: unknown;
  /** JSONではなく、バイト列をそのまま本文にする（メディアのアップロード）。 */
  rawBody?: { contentType: string; bytes: Buffer };
  extras?: RequestExtras;
  /** auth を使わずに送るCookie（ログインの前から持っているCookie）。 */
  cookies?: Map<string, string>;
}

function cookieHeader(cookies: Map<string, string>): string | null {
  if (cookies.size === 0) return null;
  return [...cookies.entries()].map(([k, v]) => `${k}=${v}`).join("; ");
}

/** Set-Cookie を、手元のCookieに反映する（Max-Age=0 は削除）。 */
function applySetCookies(cookies: Map<string, string>, headers: string[]): void {
  for (const h of headers) {
    const c = parseSetCookie(h);
    if (c.attributes.get("max-age") === "0" || c.value === "") cookies.delete(c.name);
    else cookies.set(c.name, c.value);
  }
}

class SetupError extends Error {
  constructor(what: string, res: HttpResponse) {
    super(`${what} に失敗しました: 状態コード ${res.status}: ${res.text.slice(0, 500)}`);
    this.name = "SetupError";
  }
}

function ensure(what: string, res: HttpResponse): HttpResponse {
  if (!isSuccess(res)) throw new SetupError(what, res);
  return res;
}

export function connectAdmin(baseUrl: string, todo: (what: string) => never): YomiClient {
  const base = baseUrl.replace(/\/+$/, "");

  async function request(method: string, path: string, options: RequestOptions = {}): Promise<HttpResponse> {
    const url = new URL(base + path);
    for (const [k, v] of Object.entries(options.extras?.query ?? {})) url.searchParams.set(k, v);
    const headers: Record<string, string> = {};
    const auth = options.auth ?? { kind: "none" };
    if (auth.kind === "session") {
      const cookie = cookieHeader(auth.session.cookies);
      if (cookie) headers["Cookie"] = cookie;
      if (auth.csrf === "valid") headers[CSRF_HEADER] = auth.session.csrfToken;
      else if (auth.csrf !== "missing") headers[CSRF_HEADER] = auth.csrf.token;
    } else if (auth.kind === "bearer") {
      headers["Authorization"] = `Bearer ${auth.token}`;
    } else if (options.cookies) {
      const cookie = cookieHeader(options.cookies);
      if (cookie) headers["Cookie"] = cookie;
    }
    if (options.extras?.clientIp) headers["X-Forwarded-For"] = options.extras.clientIp;
    let body: string | Blob | undefined;
    if (options.body !== undefined) {
      headers["Content-Type"] = "application/json";
      body = JSON.stringify(options.body);
    } else if (options.rawBody !== undefined) {
      headers["Content-Type"] = options.rawBody.contentType;
      body = new Blob([new Uint8Array(options.rawBody.bytes)]);
    }
    Object.assign(headers, options.extras?.headers ?? {});
    const res = await fetch(url, { method, headers, body, redirect: "manual" });
    return toHttpResponse(res);
  }

  const session = (s: Session): Auth => ({ kind: "session", session: s, csrf: "valid" });

  /** パスキーの照合に使うオリジン（CMSの公開URL）。 */
  const publicOrigin = new URL(validServerConfigFromEnv().publicUrl).origin;
  /** このドライバーで登録したパスキー（認証器の秘密鍵を含む）。 */
  const softPasskeys = new Map<string, SoftPasskey>();

  /**
   * SSOを始めた応答から、IdPの認可のURLへ移動し、IdPが戻す先（CMSの戻り先）へ移動する。戻り先の応答を返す。
   * IdPが戻す先は公開URLなので、パスと問い合わせだけを、つないでいるサーバーに送る。
   */
  async function viaIdp(start: HttpResponse, cookies: Map<string, string>, extras?: RequestExtras): Promise<HttpResponse> {
    const { authorizationUrl } = JSON.parse(start.text) as { authorizationUrl?: unknown };
    if (typeof authorizationUrl !== "string") throw new SetupError("SSOの開始の応答の読み取り", start);
    const idp = await toHttpResponse(await fetch(authorizationUrl, { redirect: "manual" }));
    const location = idp.headers.get("location");
    if (idp.status < 300 || idp.status >= 400 || !location) throw new SetupError("擬似IdPの認可", idp);
    const back = new URL(location);
    const res = await request("GET", back.pathname + back.search, { cookies, extras });
    applySetCookies(cookies, res.setCookies);
    return res;
  }

  async function loginWithOidcRaw(options: LoginOptions = {}): Promise<{ response: HttpResponse; session: Session }> {
    const cookies = new Map(options.cookies ?? []);
    const setCookieHeaders: string[] = [];
    const result = (response: HttpResponse, csrfToken = "") => ({ response, session: { cookies, csrfToken, setCookieHeaders } });
    const start = await request("POST", `${ADMIN}/session/oidc/start`, { body: {}, cookies, extras: options.extras });
    applySetCookies(cookies, start.setCookies);
    setCookieHeaders.push(...start.setCookies);
    if (!isSuccess(start)) return result(start);
    const back = await viaIdp(start, cookies, options.extras);
    setCookieHeaders.push(...back.setCookies);
    if (!back.setCookies.some((c) => parseSetCookie(c).name === "__Host-yomi_session")) return result(back);
    // 戻った画面が、CSRFトークンを一度だけ受け取る
    const done = await request("POST", `${ADMIN}/session/oidc/complete`, { cookies, extras: options.extras });
    applySetCookies(cookies, done.setCookies);
    setCookieHeaders.push(...done.setCookies);
    const parsed = isSuccess(done) ? (JSON.parse(done.text) as { csrfToken?: unknown }) : {};
    return result(done, typeof parsed.csrfToken === "string" ? parsed.csrfToken : "");
  }

  /**
   * TOTPのコードを作る。サーバーは同じ時間の区切りのコードを二度受け付けないので、
   * 秘密ごとに使った区切りを覚え、前後1区切りの中でまだ使っていないものを使う（なければ次の区切りまで待つ）。
   */
  const usedSteps = new Map<string, number>();
  // 同じ時間の区切りのコードは一度しか使えない（サーバーが再利用を拒む）。テストのファイルごとに別のプロセスになるので、
  // 使った区切りはプロセスの外（一時ディレクトリのファイル）にも記録し、プロセスをまたいで同じコードを使わないようにする。
  const stepFile = (secret: string) => join(tmpdir(), `yomi-inv-totp-${createHash("sha256").update(secret).digest("hex").slice(0, 16)}`);
  const lastUsedStep = (secret: string): number => {
    let fromFile = -Infinity;
    try {
      fromFile = Number(readFileSync(stepFile(secret), "utf8")) || -Infinity;
    } catch {
      // まだ記録がない
    }
    return Math.max(usedSteps.get(secret) ?? -Infinity, fromFile);
  };
  async function nextTotpCode(secret: string): Promise<string> {
    for (;;) {
      const current = Math.floor(Date.now() / 30_000);
      const last = lastUsedStep(secret);
      const step = [current - 1, current, current + 1].find((s) => s > last);
      if (step !== undefined) {
        usedSteps.set(secret, step);
        writeFileSync(stepFile(secret), String(step), { mode: 0o600 });
        return totp(secret, new Date(step * 30_000));
      }
      await new Promise((r) => setTimeout(r, (current + 1) * 30_000 - Date.now() + 50));
    }
  }

  interface LoginFlow {
    /** 最後の応答。 */
    response: HttpResponse;
    cookies: Map<string, string>;
    setCookieHeaders: string[];
  }

  /** ログインの段階を順に行う。passwordOnly なら1段目（パスワード）だけ。 */
  async function loginFlow(creds: Credentials, options: LoginOptions, passwordOnly: boolean): Promise<LoginFlow> {
    const cookies = new Map(options.cookies ?? []);
    const setCookieHeaders: string[] = [];
    const first = await request("POST", `${ADMIN}/session`, {
      body: { email: creds.email, password: creds.password },
      cookies,
      extras: options.extras,
    });
    applySetCookies(cookies, first.setCookies);
    setCookieHeaders.push(...first.setCookies);
    const needsMfa = isSuccess(first) && (JSON.parse(first.text) as { mfaRequired?: unknown }).mfaRequired === true;
    if (!needsMfa || passwordOnly || !creds.totpSecret) return { response: first, cookies, setCookieHeaders };

    const second = await request("POST", `${ADMIN}/session/mfa`, {
      body: { totpCode: await nextTotpCode(creds.totpSecret) },
      cookies,
      extras: options.extras,
    });
    applySetCookies(cookies, second.setCookies);
    setCookieHeaders.push(...second.setCookies);
    return { response: second, cookies, setCookieHeaders };
  }

  async function loginRaw(creds: Credentials, options: LoginOptions = {}): Promise<HttpResponse> {
    return (await loginFlow(creds, options, false)).response;
  }

  function sessionOf(flow: LoginFlow): Session {
    const parsed = JSON.parse(flow.response.text) as { csrfToken?: unknown };
    return {
      cookies: flow.cookies,
      csrfToken: typeof parsed.csrfToken === "string" ? parsed.csrfToken : "",
      setCookieHeaders: flow.setCookieHeaders,
    };
  }

  async function login(creds: Credentials, options: LoginOptions = {}): Promise<Session> {
    const flow = await loginFlow(creds, options, false);
    ensure("ログイン", flow.response);
    const session = sessionOf(flow);
    if (session.csrfToken === "") throw new SetupError("ログイン（多要素認証まで完了しなかった）", flow.response);
    return session;
  }

  async function loginPasswordOnly(creds: Credentials, options: LoginOptions = {}): Promise<Session> {
    const flow = await loginFlow(creds, options, true);
    ensure("ログインのパスワードの段階", flow.response);
    return sessionOf(flow);
  }

  function reauthenticateRaw(s: Session, method: { password?: string; totpCode?: string }): Promise<HttpResponse> {
    return request("POST", `${ADMIN}/session/reauth`, { auth: session(s), body: method });
  }

  /** 再認証がまだ十分に有効なら、改めて行わない（多要素認証のコードは30秒に1つしか使えないため）。 */
  const REAUTH_MARGIN_SECONDS = 120;
  async function reauthenticate(s: Session, creds: Credentials): Promise<void> {
    const me = await meRaw(session(s));
    if (isSuccess(me)) {
      const { reauthSecondsLeft } = JSON.parse(me.text) as { reauthSecondsLeft?: unknown };
      if (typeof reauthSecondsLeft === "number" && reauthSecondsLeft >= REAUTH_MARGIN_SECONDS) return;
    }
    const method = creds.totpSecret ? { totpCode: await nextTotpCode(creds.totpSecret) } : { password: creds.password };
    ensure("再認証", await reauthenticateRaw(s, method));
  }

  function changePasswordRaw(s: Session, currentPassword: string, newPassword: string): Promise<HttpResponse> {
    return request("POST", `${ADMIN}/me/password`, { auth: session(s), body: { currentPassword, newPassword } });
  }

  const meRaw = (auth: Auth, extras?: RequestExtras): Promise<HttpResponse> => request("GET", `${ADMIN}/me`, { auth, extras });
  const schemaInfoRaw = (auth: Auth, workspaceId: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("GET", `${ADMIN}/workspaces/${workspaceId}/schema`, { auth, extras });
  const deployTemplateRaw = (
    auth: Auth,
    template: TemplateSpec,
    options: { allowDestructive?: boolean; allowBrokenQueries?: boolean } = {},
    extras?: RequestExtras,
  ): Promise<HttpResponse> =>
    request("POST", `${ADMIN}/templates/${encodeURIComponent(template.name)}/deploy`, {
      auth,
      extras,
      body: { manifest: toManifest(template), allowDestructive: options.allowDestructive, allowBrokenQueries: options.allowBrokenQueries },
    });
  // --- 記事（BASE_TEMPLATE の最初のコンテンツタイプを使う） ---
  const CONTENT_TYPE = BASE_TEMPLATE.contentTypes[0]!.name;
  const entriesPath = (workspaceId: string) => `${ADMIN}/workspaces/${workspaceId}/entries`;
  const entryFields = (input: Partial<EntryInput>, forCreate: boolean): Record<string, unknown> => {
    const out: Record<string, unknown> = { ...(input.fields ?? {}) };
    if (input.title !== undefined) out["title"] = input.title;
    if (input.body !== undefined) out["body"] = input.body;
    if (input.slug !== undefined) out["slug"] = input.slug;
    else if (forCreate) out["slug"] = `inv-${randomUUID()}`;
    return out;
  };
  const createEntryRaw = (auth: Auth, workspaceId: string, input: EntryInput, extras?: RequestExtras): Promise<HttpResponse> =>
    request("POST", entriesPath(workspaceId), {
      auth,
      extras,
      body: { contentType: CONTENT_TYPE, locale: input.locale, fields: entryFields(input, true) },
    });
  const getEntryRaw = (auth: Auth, workspaceId: string, entryId: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("GET", `${entriesPath(workspaceId)}/${entryId}`, { auth, extras });
  const saveDraftRaw = (
    auth: Auth,
    workspaceId: string,
    entryId: string,
    changes: Partial<EntryInput>,
    revision: string,
    extras?: RequestExtras,
    lockId?: string,
  ): Promise<HttpResponse> =>
    request("PUT", `${entriesPath(workspaceId)}/${entryId}/draft`, {
      auth,
      extras,
      body: { revision: Number(revision), fields: entryFields(changes, false), ...(lockId === undefined ? {} : { lockId }) },
    });
  // 編集のロック（M2。server/src/http/admin_locks.rs）。応答の lockId を保存に付ける
  const lockPath = (workspaceId: string, entryId: string) => `${entriesPath(workspaceId)}/${entryId}/edit-lock`;
  const lockIdOf = (what: string, res: HttpResponse): string => {
    const id = (JSON.parse(ensure(what, res).text) as { lockId?: unknown }).lockId;
    if (typeof id !== "string") throw new SetupError(what, res);
    return id;
  };
  const toDraft = (res: HttpResponse): Draft => {
    const d = JSON.parse(res.text) as { revision?: unknown; fields?: Record<string, unknown> };
    const fields = d.fields ?? {};
    if (typeof d.revision !== "number" || typeof fields["slug"] !== "string") throw new SetupError("下書きの読み取り", res);
    const { title, body, slug, ...rest } = fields;
    return { revision: String(d.revision), title: typeof title === "string" ? title : "", body: body as TiptapDoc, slug: slug as string, fields: rest };
  };

  // --- 公開・版 ---
  // 公開は、同じ認証で読んだ下書きのリビジョン番号を付けて送る（読めなければ、合わないリビジョン番号で送る）
  const publishRaw = async (
    auth: Auth,
    workspaceId: string,
    entryId: string,
    options: { correction?: boolean; note?: string } = {},
    extras?: RequestExtras,
  ): Promise<HttpResponse> => {
    const current = await getEntryRaw(auth, workspaceId, entryId, extras);
    const revision = isSuccess(current) ? (JSON.parse(current.text) as { revision: number }).revision : 0;
    // 訂正の説明は自由記述として送る（定型文を選ぶ操作は管理画面のもの。server/src/http/admin_corrections.rs）
    const note = options.note === undefined ? {} : { correctionNote: { text: options.note } };
    return request("POST", `${entriesPath(workspaceId)}/${entryId}/publish`, {
      auth,
      extras,
      body: { revision, correction: options.correction ?? false, ...note },
    });
  };
  // 予約公開（M2。server/src/http/admin_scheduling.rs）。公開と同じく、同じ認証で読んだ下書きのリビジョン番号を付けて送る
  const scheduleRaw = async (
    auth: Auth,
    workspaceId: string,
    entryId: string,
    at: Date,
    options: { correction?: boolean; note?: string } = {},
  ): Promise<HttpResponse> => {
    const current = await getEntryRaw(auth, workspaceId, entryId);
    const revision = isSuccess(current) ? (JSON.parse(current.text) as { revision: number }).revision : 0;
    const note = options.note === undefined ? {} : { correctionNote: { text: options.note } };
    return request("PUT", `${entriesPath(workspaceId)}/${entryId}/schedule`, {
      auth,
      body: { revision, publishAt: at.toISOString(), ...(options.correction === undefined ? {} : { correction: options.correction }), ...note },
    });
  };
  // 訂正の説明を書き直す（版ごと）。確認済みを記録する（記事ごと）
  const editCorrectionNoteRaw = (auth: Auth, workspaceId: string, entryId: string, versionId: string, note: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("PUT", `${entriesPath(workspaceId)}/${entryId}/versions/${versionId}/correction-note`, { auth, extras, body: { text: note } });
  const markVerifiedRaw = (auth: Auth, workspaceId: string, entryId: string, memo: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("POST", `${entriesPath(workspaceId)}/${entryId}/verify`, { auth, extras, body: { memo } });
  const unpublishRaw = (auth: Auth, workspaceId: string, entryId: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("POST", `${entriesPath(workspaceId)}/${entryId}/unpublish`, { auth, extras });
  const deleteEntryRaw = (auth: Auth, workspaceId: string, entryId: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("DELETE", `${entriesPath(workspaceId)}/${entryId}`, { auth, extras });
  const versionsRaw = (auth: Auth, workspaceId: string, entryId: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("GET", `${entriesPath(workspaceId)}/${entryId}/versions`, { auth, extras });
  const versionRaw = (auth: Auth, workspaceId: string, entryId: string, versionId: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("GET", `${entriesPath(workspaceId)}/${entryId}/versions/${versionId}`, { auth, extras });
  const urlHistoryRaw = (auth: Auth, workspaceId: string, entryId: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("GET", `${entriesPath(workspaceId)}/${entryId}/url-history`, { auth, extras });

  // --- 公開設定のスイッチ（server/src/content/settings.rs） ---
  const settingsPath = (workspaceId: string, entryId: string) => `${entriesPath(workspaceId)}/${entryId}/publish-settings`;
  const getPublishSettingsRaw = (auth: Auth, workspaceId: string, entryId: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("GET", settingsPath(workspaceId, entryId), { auth, extras });
  /** 同じ認証で今の値を読み、「外部AI向けの素材に含める」を反対にする（読めなければ、今の値を含める側と仮定する）。 */
  const togglePublishSettingsRaw = async (auth: Auth, workspaceId: string, entryId: string, extras?: RequestExtras): Promise<HttpResponse> => {
    const current = await getPublishSettingsRaw(auth, workspaceId, entryId, extras);
    const include = isSuccess(current) ? (JSON.parse(current.text) as { includeInAiMaterials?: unknown }).includeInAiMaterials !== false : true;
    return request("PATCH", settingsPath(workspaceId, entryId), { auth, extras, body: { includeInAiMaterials: !include } });
  };

  // --- 重い権限 ---
  const memberPermissionList = async (s: Session, workspaceId: string, userId: string, key: "permissions" | "grants"): Promise<HeavyPermission[]> => {
    const res = ensure("所属者の権限の取得", await request("GET", `${ADMIN}/workspaces/${workspaceId}/members/${userId}`, { auth: session(s) }));
    const list = (JSON.parse(res.text) as Record<string, unknown>)[key];
    if (!Array.isArray(list) || !list.every((g) => typeof g === "string")) throw new SetupError("所属者の権限の読み取り", res);
    return list as HeavyPermission[];
  };

  // --- 版の本文の抹消 ---
  const eraseCandidatesRaw = (auth: Auth, workspaceId: string, entryId: string, versionId: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("GET", `${entriesPath(workspaceId)}/${entryId}/versions/${versionId}/erase-candidates`, { auth, extras });
  const eraseVersionRaw = (auth: Auth, workspaceId: string, entryId: string, versionId: string, deleteMediaIds: string[], extras?: RequestExtras): Promise<HttpResponse> =>
    request("POST", `${entriesPath(workspaceId)}/${entryId}/versions/${versionId}/erase`, { auth, extras, body: { deleteMediaIds } });

  // --- 翻訳 ---
  const createTranslationRaw = (auth: Auth, workspaceId: string, entryId: string, locale: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("POST", `${entriesPath(workspaceId)}/${entryId}/translations`, { auth, extras, body: { locale } });
  const translationInfoRaw = (auth: Auth, workspaceId: string, entryId: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("GET", `${entriesPath(workspaceId)}/${entryId}/translation`, { auth, extras });
  const sourceDiffRaw = (auth: Auth, workspaceId: string, entryId: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("GET", `${entriesPath(workspaceId)}/${entryId}/translation/source-diff`, { auth, extras });
  const markSourceReflectedRaw = (auth: Auth, workspaceId: string, entryId: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("POST", `${entriesPath(workspaceId)}/${entryId}/translation/reflected`, { auth, extras });
  const requestReviewRaw = (auth: Auth, workspaceId: string, entryId: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("POST", `${entriesPath(workspaceId)}/${entryId}/review-request`, { auth, extras, body: {} });
  const todoRaw = (auth: Auth, workspaceId: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("GET", `${ADMIN}/workspaces/${workspaceId}/todo`, { auth, extras });
  const setTranslationSourceRaw = (auth: Auth, workspaceId: string, entryId: string, sourceEntryId: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("PUT", `${entriesPath(workspaceId)}/${entryId}/translation/source`, { auth, extras, body: { sourceEntryId } });

  // --- メディア ---
  // ファイルのバイト列をそのまま本文にして送る。ファイル名はサーバーに送らない（サーバーはランダムな名前で保存する）
  const mediaPath = (workspaceId: string) => `${ADMIN}/workspaces/${workspaceId}/media`;
  const uploadMediaRaw = (auth: Auth, workspaceId: string, file: UploadFile, extras?: RequestExtras): Promise<HttpResponse> =>
    request("POST", mediaPath(workspaceId), { auth, extras, rawBody: { contentType: file.contentType, bytes: file.bytes } });
  const setAnswerSettingsRaw = (auth: Auth, workspaceId: string, settings: AnswerSettings, extras?: RequestExtras): Promise<HttpResponse> =>
    request("PUT", `${ADMIN}/workspaces/${workspaceId}/answer-settings`, { auth, extras, body: settings });
  const setQuestionLogSettingsRaw = (auth: Auth, workspaceId: string, settings: QuestionLogSettings, extras?: RequestExtras): Promise<HttpResponse> =>
    request("PUT", `${ADMIN}/workspaces/${workspaceId}/question-log/settings`, { auth, extras, body: settings });
  const questionLogRaw = (auth: Auth, workspaceId: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("GET", `${ADMIN}/workspaces/${workspaceId}/question-log`, { auth, extras });
  const setWorkspaceLimitsRaw = (auth: Auth, workspaceId: string, limits: WorkspaceLimits, extras?: RequestExtras): Promise<HttpResponse> => {
    // 書いた項目だけを送る（サーバーは、書いた項目だけを変える）
    const body: Record<string, number | boolean | null> = {};
    if ("llmOverride" in limits) body["llmOverride"] = limits.llmOverride ?? false;
    if ("mediaBytes" in limits) body["mediaBytes"] = limits.mediaBytes ?? null;
    if ("embeddingTokensPerMonth" in limits) body["embeddingTokensPerMonth"] = limits.embeddingTokensPerMonth ?? null;
    if ("embeddingTokensPerDay" in limits) body["embeddingTokensPerDay"] = limits.embeddingTokensPerDay ?? null;
    for (const k of [
      "answerInputTokensPerMonth",
      "answerInputTokensPerDay",
      "answerOutputTokensPerMonth",
      "answerOutputTokensPerDay",
      "assistInputTokensPerMonth",
      "assistInputTokensPerDay",
      "assistOutputTokensPerMonth",
      "assistOutputTokensPerDay",
    ] as const) {
      if (k in limits) body[k] = limits[k] ?? null;
    }
    return request("PUT", `${ADMIN}/workspaces/${workspaceId}/limits`, { auth, extras, body });
  };
  const setEmbeddingBudgetRaw = (auth: Auth, workspaceId: string, budget: EmbeddingBudget, extras?: RequestExtras): Promise<HttpResponse> =>
    request("PUT", `${ADMIN}/workspaces/${workspaceId}/budgets/embedding`, { auth, extras, body: budget });
  const setAssistBudgetRaw = (auth: Auth, workspaceId: string, budget: AssistBudget, extras?: RequestExtras): Promise<HttpResponse> =>
    request("PUT", `${ADMIN}/workspaces/${workspaceId}/budgets/assist`, { auth, extras, body: budget });
  const setAssistSettingsRaw = (auth: Auth, workspaceId: string, settings: AssistSettings, extras?: RequestExtras): Promise<HttpResponse> =>
    request("PUT", `${ADMIN}/workspaces/${workspaceId}/assist-settings`, { auth, extras, body: settings });
  const conflictsPath = (workspaceId: string, entryId: string) => `${entriesPath(workspaceId)}/${entryId}/assist/conflicts`;
  // 操作の一覧で使う小さな画像（1×1のPNG）
  const TINY_PNG = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "base64");

  // --- 配信API（GraphQL） ---
  // 名前はサーバーの決まりに合わせる: コンテンツタイプ blog_post → 型 BlogPost、blogPost・blogPostBySlug・blogPostList
  const POST_TYPE = postType(BASE_TEMPLATE);
  const camel = (name: string): string => name.split("_").filter(Boolean).map((p, i) => (i === 0 ? p : p[0]!.toUpperCase() + p.slice(1))).join("");
  const SINGLE = camel(POST_TYPE.name);
  const LIST = `${SINGLE}List`;
  // 回答の出典と、回答（まとめて返すものと、ストリーミングの最終判定）で選ぶ項目（M4）
  const ANSWER_SOURCE_FIELDS = "entryId locale url anchor breadcrumb versionId lastReviewedAt reviewExpired";
  const ANSWER_FIELDS = `status text truncated sources { ${ANSWER_SOURCE_FIELDS} }`;
  const DEFAULT_LOCALE = BASE_TEMPLATE.defaultLocale;
  const selection = (fields?: string[]): string => (fields ? ["id", ...fields.filter((f) => f !== "id")] : ["id", "title", "slug"]).join(" ");
  // 変換した本文: 本文のフィールド <名前> に対して <名前>Html と <名前>Markdown（server/src/delivery/schema.rs）
  // 訂正履歴・削除済みアンカー・鮮度（M2。server/src/delivery/schema.rs）
  // AI向けのMarkdown版（M2。server/src/delivery/schema.rs）
  const INCLUDES: Record<"body" | "bodyHtml" | "bodyMarkdown" | "alternates" | "related" | "corrections" | "deletedAnchors" | "freshness" | "aiMarkdown" | "cover", string> = {
    // 画像のフィールド（M5。server/src/delivery/schema.rs）
    cover: "cover { url width height alt decorative }",
    aiMarkdown: "aiMarkdown",
    corrections: "corrections { correctedAt description sections }",
    deletedAnchors: "deletedAnchors { anchor deletedAt description }",
    freshness: "lastVerifiedAt reviewDueAt reviewExpired",
    body: "body",
    bodyHtml: "bodyHtml",
    bodyMarkdown: "bodyMarkdown",
    alternates: "alternates { id locale url }",
    related: "related { id title slug }",
  };
  const queryText = (spec: QuerySpec): string => {
    switch (spec.kind) {
      case "list":
        // 変数を省略したときは、既定のロケールと上限の件数
        return `query($locale: String! = ${JSON.stringify(DEFAULT_LOCALE)}, $limit: Int) { ${LIST}(locale: $locale, limit: $limit) { ${[selection(spec.fields), ...(spec.include ?? []).map((i) => INCLUDES[i])].join(" ")} } }`;
      case "byId":
        return `query($id: ID!, $locale: String) { ${SINGLE}(id: $id, locale: $locale) { ${[selection(spec.fields), ...(spec.include ?? []).map((i) => INCLUDES[i])].join(" ")} } }`;
      case "bySlug":
        return `query($slug: String!, $locale: String! = ${JSON.stringify(DEFAULT_LOCALE)}) { ${SINGLE}BySlug(slug: $slug, locale: $locale) { ${[selection(spec.fields), ...(spec.include ?? []).map((i) => INCLUDES[i])].join(" ")} } }`;
      case "deep":
        return `{ ${LIST}(locale: ${JSON.stringify(DEFAULT_LOCALE)}) { ${"related { ".repeat(spec.depth)}id${" }".repeat(spec.depth)} } }`;
      case "wide":
        return `{ ${Array.from({ length: spec.aliases }, (_, i) => `a${i}: ${LIST}(locale: ${JSON.stringify(DEFAULT_LOCALE)}) { id }`).join(" ")} }`;
      case "correctionFeed":
        return "query($locale: String!, $limit: Int) { corrections(locale: $locale, limit: $limit) { entryId url correctedAt description sections } }";
      case "redirects":
        return "{ redirects { from to status } }";
      case "aiMarkdownByUrl":
        return "query($url: String!) { aiMarkdownByUrl(url: $url) }";
      case "aiCatalog":
        return "query($locale: String!, $limit: Int) { aiCatalog(locale: $locale, limit: $limit) { entryId contentType url title locale updatedAt } }";
      // サイト内検索（M3。server/src/delivery）
      case "search":
        return `query($text: String!, $locale: String! = ${JSON.stringify(DEFAULT_LOCALE)}, $first: Int) { search(text: $text, locale: $locale, first: $first) { vectorSearch keywordSearch hits { entryId locale anchor breadcrumb excerpt score } } }`;
      case "searchWithMode":
        return `query($text: String!, $locale: String! = ${JSON.stringify(DEFAULT_LOCALE)}, $first: Int, $mode: YomiSearchMode) { search(text: $text, locale: $locale, first: $first, mode: $mode) { vectorSearch keywordSearch hits { entryId locale anchor breadcrumb excerpt score } } }`;
      // 関連記事（M3。server/src/delivery）
      case "relatedEntries":
        return `query($id: ID!, $first: Int) { ${SINGLE}(id: $id) { id relatedEntries(first: $first) { id contentType locale url title pinned score } } }`;
      // 回答（M4。server/src/delivery）。出典の項目は、まとめて・ストリーミングで同じ
      case "answer": {
        const [v, a] = spec.visitorId ? [", $visitorId: String", ", visitorId: $visitorId"] : ["", ""];
        const fields = spec.cached ? `${ANSWER_FIELDS} cached` : ANSWER_FIELDS;
        return `query($question: String!, $locale: String! = ${JSON.stringify(DEFAULT_LOCALE)}${v}) { answer(question: $question, locale: $locale${a}) { ${fields} } }`;
      }
      case "answerStream": {
        const [v, a] = spec.visitorId ? [", $visitorId: String", ", visitorId: $visitorId"] : ["", ""];
        const fields = spec.cached ? `${ANSWER_FIELDS} cached` : ANSWER_FIELDS;
        return `subscription($question: String!, $locale: String! = ${JSON.stringify(DEFAULT_LOCALE)}${v}) { answerStream(question: $question, locale: $locale${a}) { __typename ... on YomiAnswerVerdict { answerable sources { ${ANSWER_SOURCE_FIELDS} } } ... on YomiAnswerDelta { text } ... on YomiAnswerFinal { ${fields} } } }`;
      }
      case "text":
        return spec.text;
    }
  };
  /** 回答の応答の、GraphQL の列挙の値（ANSWERED など）をドライバーの値にする。 */
  const ANSWER_STATUS: Record<string, AnswerStatus> = { ANSWERED: "answered", NO_INFORMATION: "noInformation", VIOLATION_DETECTED: "violationDetected" };
  const toAnswerResult = (r: Record<string, unknown>): AnswerResult => ({
    status: ANSWER_STATUS[String(r["status"])] ?? (String(r["status"]) as AnswerStatus),
    text: r["text"] as string,
    truncated: r["truncated"] as boolean,
    sources: r["sources"] as AnswerSource[],
    ...(typeof r["cached"] === "boolean" ? { cached: r["cached"] } : {}),
  });
  const graphqlErrors = (body: unknown): AnswerError[] => {
    const errors = (body as { errors?: unknown } | null)?.errors;
    if (!Array.isArray(errors)) return [];
    return errors.map((e) => {
      const code = (e as { extensions?: { code?: unknown } }).extensions?.code;
      return { code: typeof code === "string" ? code : null, message: String((e as { message?: unknown }).message ?? "") };
    });
  };
  /**
   * SSE の本文を、イベントの一覧にする（GraphQL over SSE の「別々の接続」の形: `event: next` の data に GraphQL の応答、
   * 最後に `event: complete`）。
   */
  const sseEvents = (text: string): { event: string; data: unknown }[] =>
    text
      .split(/\r?\n\r?\n/u)
      .map((block) => {
        let event = "message";
        const data: string[] = [];
        for (const line of block.split(/\r?\n/u)) {
          if (line.startsWith("event:")) event = line.slice(6).trim();
          else if (line.startsWith("data:")) data.push(line.slice(5).replace(/^ /u, ""));
        }
        return { event, raw: data.join("\n") };
      })
      .filter((e) => e.event !== "message" || e.raw !== "")
      .map((e) => ({ event: e.event, data: e.raw === "" ? null : (JSON.parse(e.raw) as unknown) }));
  const isEventStream = (res: HttpResponse) => (res.headers.get("content-type") ?? "").startsWith("text/event-stream");
  const deliveryAuth = (apiKey: string | null): Auth => (apiKey === null ? { kind: "none" } : { kind: "bearer", token: apiKey });
  // プレビュー用トークンは専用のヘッダーで送る（APIキーの Authorization とは別。server/src/delivery）
  const PREVIEW_HEADER = "X-Yomi-Preview-Token";
  const previewTokensPath = (workspaceId: string, entryId: string) => `${ADMIN}/workspaces/${workspaceId}/entries/${entryId}/preview-tokens`;
  const issuePreviewTokenRaw = (auth: Auth, workspaceId: string, entryId: string, ttlSeconds?: number, extras?: RequestExtras): Promise<HttpResponse> =>
    request("POST", previewTokensPath(workspaceId, entryId), { auth, extras, body: ttlSeconds === undefined ? {} : { ttlSeconds } });
  const revokePreviewTokenRaw = (auth: Auth, workspaceId: string, entryId: string, tokenId: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("DELETE", `${previewTokensPath(workspaceId, entryId)}/${tokenId}`, { auth, extras });
  const registerQueryRaw = (workspaceIdInUrl: string, apiKey: string | null, spec: QuerySpec): Promise<HttpResponse> =>
    request("POST", `/ws/${workspaceIdInUrl}/graphql/queries`, { auth: deliveryAuth(apiKey), body: { query: queryText(spec) } });

  // --- 監査ログ（server/src/http/admin_audit.rs） ---
  const auditPath = (workspaceId: string | null) => (workspaceId === null ? `${ADMIN}/audit-log` : `${ADMIN}/workspaces/${workspaceId}/audit-log`);
  const auditPageRaw = (auth: Auth, workspaceId: string | null, before?: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("GET", auditPath(workspaceId), { auth, extras: { ...extras, query: { ...(extras?.query ?? {}), limit: "500", ...(before ? { before } : {}) } } });
  const auditFilterQuery = (f?: AuditFilter): Record<string, string> => ({
    ...(f?.action ? { action: f.action } : {}),
    ...(f?.actorUserId ? { actor: f.actorUserId } : {}),
    ...(f?.from ? { from: f.from.toISOString() } : {}),
    ...(f?.to ? { to: f.to.toISOString() } : {}),
  });
  const auditLogRaw = (auth: Auth, workspaceId: string | null, filter?: AuditFilter, extras?: RequestExtras): Promise<HttpResponse> =>
    request("GET", auditPath(workspaceId), { auth, extras: { ...extras, query: { ...(extras?.query ?? {}), ...auditFilterQuery(filter) } } });
  const exportAuditLogRaw = (auth: Auth, workspaceId: string | null, filter?: AuditFilter, extras?: RequestExtras): Promise<HttpResponse> =>
    request("GET", `${auditPath(workspaceId)}/export`, { auth, extras: { ...extras, query: { ...(extras?.query ?? {}), ...auditFilterQuery(filter) } } });
  const auditSinksPath = (workspaceId: string) => `${ADMIN}/workspaces/${workspaceId}/audit-sinks`;
  const createAuditSinkRaw = (auth: Auth, workspaceId: string, url: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("POST", auditSinksPath(workspaceId), { auth, extras, body: { url } });

  // --- Webhook ---
  const webhooksPath = (workspaceId: string) => `${ADMIN}/workspaces/${workspaceId}/webhooks`;
  const createWebhookRaw = (auth: Auth, workspaceId: string, url: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("POST", webhooksPath(workspaceId), { auth, extras, body: { url } });
  const testWebhookRaw = (auth: Auth, workspaceId: string, webhookId: string, extras?: RequestExtras): Promise<HttpResponse> =>
    request("POST", `${webhooksPath(workspaceId)}/${webhookId}/test`, { auth, extras });
  /** 同じ認証で送信先の一覧を読み、最初のもののIDを返す（読めない・ないときは存在しないID）。 */
  const firstWebhookId = async (auth: Auth, workspaceId: string, extras?: RequestExtras): Promise<string> => {
    const list = await request("GET", webhooksPath(workspaceId), { auth, extras });
    return (isSuccess(list) ? (JSON.parse(list.text) as { id: string }[])[0]?.id : undefined) ?? randomUUID();
  };
  // 操作の一覧で登録する送信先。テストの実行環境の宛先（例外として許可済み）の、何も待ち受けていないポート（9: discard）にする。
  // テストの実行環境の宛先がなければ、名前解決できない宛先にする（登録は断られる）
  const OPERATION_WEBHOOK_URL = process.env["YOMI_TEST_HARNESS_ALLOWED_HOST"]
    ? `http://${process.env["YOMI_TEST_HARNESS_ALLOWED_HOST"]}:9/inv-operation`
    : "https://inv-operation.invalid/";

  // 操作の一覧で使うテンプレート（何度デプロイしても同じ内容なので、版は増えない）
  const OPERATION_TEMPLATE = cloneTemplate("inv-operations");
  const loginHistoryRaw = (auth: Auth, extras?: RequestExtras): Promise<HttpResponse> =>
    request("GET", `${ADMIN}/me/login-history`, { auth, extras });

  const op = (
    name: string,
    mutating: boolean,
    scope: AdminOperation["scope"],
    call: (auth: Auth, target: OperationTarget, extras?: RequestExtras) => Promise<HttpResponse>,
  ): AdminOperation => ({ name, mutating, scope, call });

  // 管理APIの操作の一覧（サーバーにある操作だけ。操作を作ったら、ここにも足す）
  const adminOperations: AdminOperation[] = [
    op("me", false, "self", (auth, _t, extras) => meRaw(auth, extras)),
    // 言語は既定の言語（英語）にする
    op("updateMe", true, "self", (auth, _t, extras) => request("PATCH", `${ADMIN}/me`, { auth, extras, body: { locale: "en" } })),
    op("listMySessions", false, "self", (auth, _t, extras) => request("GET", `${ADMIN}/me/sessions`, { auth, extras })),
    op("getMyLoginHistory", false, "self", (auth, _t, extras) => loginHistoryRaw(auth, extras)),
    op("listMyIdentities", false, "self", (auth, _t, extras) => request("GET", `${ADMIN}/me/identities`, { auth, extras })),
    op("deploySchema", true, "instance", (auth, _t, extras) => deployTemplateRaw(auth, OPERATION_TEMPLATE, {}, extras)),
    op("getSchemaInfo", false, "workspace", (auth, t, extras) => schemaInfoRaw(auth, t.workspaceId, extras)),
    op("listEntries", false, "workspace", (auth, t, extras) => request("GET", entriesPath(t.workspaceId), { auth, extras })),
    op("getEntry", false, "workspace", (auth, t, extras) => getEntryRaw(auth, t.workspaceId, t.entryId, extras)),
    op("createEntry", true, "workspace", (auth, t, extras) =>
      createEntryRaw(auth, t.workspaceId, { title: "inv-op", body: { type: "doc", content: [] } }, extras),
    ),
    // 同じ認証で最新のリビジョン番号を読み、何も変えずに保存する（読めなければ、合わないリビジョン番号で送る）
    op("saveDraft", true, "workspace", async (auth, t, extras) => {
      const current = await getEntryRaw(auth, t.workspaceId, t.entryId, extras);
      const revision = isSuccess(current) ? String((JSON.parse(current.text) as { revision: number }).revision) : "0";
      return saveDraftRaw(auth, t.workspaceId, t.entryId, {}, revision, extras);
    }),
    op("publishEntry", true, "workspace", (auth, t, extras) => publishRaw(auth, t.workspaceId, t.entryId, {}, extras)),
    op("publishAsCorrection", true, "workspace", (auth, t, extras) => publishRaw(auth, t.workspaceId, t.entryId, { correction: true }, extras)),
    op("unpublishEntry", true, "workspace", (auth, t, extras) => unpublishRaw(auth, t.workspaceId, t.entryId, extras)),
    // 対象は存在しない版（舞台の記事を変えないため）
    op("editCorrectionNote", true, "workspace", (auth, t, extras) => editCorrectionNoteRaw(auth, t.workspaceId, t.entryId, randomUUID(), "inv-op", extras)),
    op("markVerified", true, "workspace", (auth, t, extras) => markVerifiedRaw(auth, t.workspaceId, t.entryId, "inv-op", extras)),
    // 対象は存在しない記事（舞台の記事を壊さないため）
    op("deleteEntry", true, "workspace", (auth, t, extras) => deleteEntryRaw(auth, t.workspaceId, randomUUID(), extras)),
    op("getPublishSettings", false, "workspace", (auth, t, extras) => getPublishSettingsRaw(auth, t.workspaceId, t.entryId, extras)),
    op("changePublishSettings", true, "workspace", (auth, t, extras) => togglePublishSettingsRaw(auth, t.workspaceId, t.entryId, extras)),
    // 取り込み（server/src/http/admin_import.rs）。新しいスラッグの記事を1件だけ取り込む
    op("importEntries", true, "workspace", (auth, t, extras) => {
      const slug = `inv-op-import-${randomUUID().slice(0, 8)}`;
      return request("POST", `${ADMIN}/workspaces/${t.workspaceId}/imports`, {
        auth,
        extras,
        body: { entries: [{ key: "op.md", locale: DEFAULT_LOCALE, originalUrl: `https://old.inv.test/${slug}`, fields: { title: "inv-op", slug } }] },
      });
    }),
    op("listEntryVersions", false, "workspace", (auth, t, extras) => versionsRaw(auth, t.workspaceId, t.entryId, extras)),
    // 同じ認証で版の一覧を読み、最新の版を読む（読めなければ、存在しない版を読む）
    op("getEntryVersion", false, "workspace", async (auth, t, extras) => {
      const list = await versionsRaw(auth, t.workspaceId, t.entryId, extras);
      const first = isSuccess(list) ? (JSON.parse(list.text) as { id: string }[])[0]?.id : undefined;
      return versionRaw(auth, t.workspaceId, t.entryId, first ?? randomUUID(), extras);
    }),
    // テンプレートの最後のロケールの翻訳を作る（もうあれば 409）
    op("createTranslation", true, "workspace", (auth, t, extras) =>
      createTranslationRaw(auth, t.workspaceId, t.entryId, BASE_TEMPLATE.locales[BASE_TEMPLATE.locales.length - 1]!, extras),
    ),
    op("getTranslationInfo", false, "workspace", (auth, t, extras) => translationInfoRaw(auth, t.workspaceId, t.entryId, extras)),
    op("issuePreviewToken", true, "workspace", (auth, t, extras) => issuePreviewTokenRaw(auth, t.workspaceId, t.entryId, undefined, extras)),
    // 対象は存在しないトークン
    op("revokePreviewToken", true, "workspace", (auth, t, extras) => revokePreviewTokenRaw(auth, t.workspaceId, t.entryId, randomUUID(), extras)),
    op("exportAuditLog", false, "workspace", (auth, t, extras) => exportAuditLogRaw(auth, t.workspaceId, undefined, extras)),
    op("listAuditSinks", false, "workspace", (auth, t, extras) => request("GET", auditSinksPath(t.workspaceId), { auth, extras })),
    // 宛先は届かないアドレス（送信はジョブが行い、失敗しても舞台に影響しない）
    op("createAuditSink", true, "workspace", (auth, t, extras) => createAuditSinkRaw(auth, t.workspaceId, "https://audit-sink.invalid/inv-op", extras)),
    // 対象は存在しない書き出し先
    op("deleteAuditSink", true, "workspace", (auth, t, extras) => request("DELETE", `${auditSinksPath(t.workspaceId)}/${randomUUID()}`, { auth, extras })),
    op("getTodo", false, "workspace", (auth, t, extras) => todoRaw(auth, t.workspaceId, extras)),
    op("getSourceDiff", false, "workspace", (auth, t, extras) => sourceDiffRaw(auth, t.workspaceId, t.entryId, extras)),
    op("markSourceReflected", true, "workspace", (auth, t, extras) => markSourceReflectedRaw(auth, t.workspaceId, t.entryId, extras)),
    // 元の記事は存在しない記事（舞台の翻訳グループを変えないため）
    op("setTranslationSource", true, "workspace", (auth, t, extras) => setTranslationSourceRaw(auth, t.workspaceId, t.entryId, randomUUID(), extras)),
    op("listMedia", false, "workspace", (auth, t, extras) => request("GET", mediaPath(t.workspaceId), { auth, extras })),
    op("uploadMedia", true, "workspace", (auth, t, extras) =>
      uploadMediaRaw(auth, t.workspaceId, { filename: "op.png", bytes: TINY_PNG, contentType: "image/png" }, extras),
    ),
    // 対象は存在しないメディア（舞台のメディアを壊さないため）
    op("deleteMedia", true, "workspace", (auth, t, extras) => request("DELETE", `${mediaPath(t.workspaceId)}/${randomUUID()}`, { auth, extras })),
    op("getWorkspaceLimits", false, "workspace", (auth, t, extras) => request("GET", `${ADMIN}/workspaces/${t.workspaceId}/limits`, { auth, extras })),
    // 上限は既定値のまま（null）にする
    op("setWorkspaceLimits", true, "instance", (auth, t, extras) => setWorkspaceLimitsRaw(auth, t.workspaceId, {}, extras)),
    // 埋め込みの作成の予算（重い権限 manageLlm）。値は天井の範囲に必ず収まる 0
    op("getEmbeddingBudget", false, "workspace", (auth, t, extras) => request("GET", `${ADMIN}/workspaces/${t.workspaceId}/budgets/embedding`, { auth, extras })),
    op("setEmbeddingBudget", true, "workspace", (auth, t, extras) => setEmbeddingBudgetRaw(auth, t.workspaceId, { tokensPerMonth: 0, tokensPerDay: 0 }, extras)),
    // 回答の語り口と定型文（M4。重い権限 manageLlm）。何も変えない変更を送る
    op("getAnswerSettings", false, "workspace", (auth, t, extras) => request("GET", `${ADMIN}/workspaces/${t.workspaceId}/answer-settings`, { auth, extras })),
    op("setAnswerSettings", true, "workspace", (auth, t, extras) => setAnswerSettingsRaw(auth, t.workspaceId, {}, extras)),
    // LLMの接続設定の上書き（M4。重い権限 manageLlm）。変更・削除は、許されていないワークスペースでは断られる
    op("getWorkspaceLlm", false, "workspace", (auth, t, extras) => request("GET", `${ADMIN}/workspaces/${t.workspaceId}/llm`, { auth, extras })),
    op("setWorkspaceLlm", true, "workspace", (auth, t, extras) =>
      request("PUT", `${ADMIN}/workspaces/${t.workspaceId}/llm`, { auth, extras, body: { endpoint: "inv-operation" } }),
    ),
    op("deleteWorkspaceLlm", true, "workspace", (auth, t, extras) => request("DELETE", `${ADMIN}/workspaces/${t.workspaceId}/llm`, { auth, extras })),
    op("startWorkspaceEmbeddingSwitch", true, "workspace", (auth, t, extras) =>
      request("POST", `${ADMIN}/workspaces/${t.workspaceId}/llm/embedding-switch`, { auth, extras }),
    ),
    // 質問ログ（M4）: 設定はワークスペースの管理者、閲覧と候補の削除は重い権限 viewQuestionLog。削除の対象は存在しない候補
    op("getQuestionLogSettings", false, "workspace", (auth, t, extras) => request("GET", `${ADMIN}/workspaces/${t.workspaceId}/question-log/settings`, { auth, extras })),
    op("setQuestionLogSettings", true, "workspace", (auth, t, extras) => setQuestionLogSettingsRaw(auth, t.workspaceId, {}, extras)),
    op("getQuestionLog", false, "workspace", (auth, t, extras) => questionLogRaw(auth, t.workspaceId, extras)),
    op("deleteQuestionCandidate", true, "workspace", (auth, t, extras) =>
      request("DELETE", `${ADMIN}/workspaces/${t.workspaceId}/question-log/candidates/${"0".repeat(64)}`, { auth, extras }),
    ),
    // 対象は存在しない版（舞台の版を壊さないため）
    op("eraseVersion", true, "workspace", (auth, t, extras) => eraseVersionRaw(auth, t.workspaceId, t.entryId, randomUUID(), [], extras)),
    op("getEraseCandidates", false, "workspace", async (auth, t, extras) => {
      const list = await versionsRaw(auth, t.workspaceId, t.entryId, extras);
      const first = isSuccess(list) ? (JSON.parse(list.text) as { id: string }[])[0]?.id : undefined;
      return eraseCandidatesRaw(auth, t.workspaceId, t.entryId, first ?? randomUUID(), extras);
    }),
    op("getUrlHistory", false, "workspace", (auth, t, extras) => urlHistoryRaw(auth, t.workspaceId, t.entryId, extras)),
    // 対象は存在しないセッション（本人の他のセッションを壊さないため）
    op("revokeMySession", true, "self", (auth, _t, extras) => request("DELETE", `${ADMIN}/me/sessions/${randomUUID()}`, { auth, extras })),
    // 現在のパスワードは誤った値にする（舞台の利用者のパスワードを変えないため）
    op("changeMyPassword", true, "self", (auth, _t, extras) =>
      request("POST", `${ADMIN}/me/password`, {
        auth,
        extras,
        body: { currentPassword: `wrong-${randomUUID()}`, newPassword: `inv-${randomUUID()}` },
      }),
    ),
    op("beginTotpSetup", true, "self", (auth, _t, extras) => request("POST", `${ADMIN}/me/mfa/totp`, { auth, extras })),
    // コードは誤った値にする（舞台の利用者の多要素認証を変えないため）
    op("confirmTotpSetup", true, "self", (auth, _t, extras) =>
      request("POST", `${ADMIN}/me/mfa/totp/confirm`, { auth, extras, body: { code: "000000" } }),
    ),
    op("regenerateRecoveryCodes", true, "self", (auth, _t, extras) => request("POST", `${ADMIN}/me/mfa/recovery-codes`, { auth, extras })),
    op("disableTotp", true, "self", (auth, _t, extras) => request("DELETE", `${ADMIN}/me/mfa/totp`, { auth, extras })),
    op("logout", true, "self", (auth, _t, extras) => request("DELETE", `${ADMIN}/session`, { auth, extras })),
    // --- ワークスペースと所属 ---
    op("listWorkspaces", false, "self", (auth, _t, extras) => request("GET", `${ADMIN}/workspaces`, { auth, extras })),
    op("createWorkspace", true, "instance", (auth, _t, extras) =>
      request("POST", `${ADMIN}/workspaces`, { auth, extras, body: { name: `inv-op-${randomUUID().slice(0, 8)}` } }),
    ),
    op("listUsers", false, "instance", (auth, _t, extras) => request("GET", `${ADMIN}/users`, { auth, extras })),
    op("getWorkspace", false, "workspace", (auth, t, extras) => request("GET", `${ADMIN}/workspaces/${t.workspaceId}`, { auth, extras })),
    op("listMembers", false, "workspace", (auth, t, extras) => request("GET", `${ADMIN}/workspaces/${t.workspaceId}/members`, { auth, extras })),
    op("getMember", false, "workspace", (auth, t, extras) =>
      request("GET", `${ADMIN}/workspaces/${t.workspaceId}/members/${t.userId}`, { auth, extras }),
    ),
    op("changeMemberRole", true, "workspace", (auth, t, extras) =>
      request("PATCH", `${ADMIN}/workspaces/${t.workspaceId}/members/${t.userId}`, { auth, extras, body: { role: "editor" } }),
    ),
    op("inviteMember", true, "workspace", (auth, t, extras) =>
      request("POST", `${ADMIN}/workspaces/${t.workspaceId}/invitations`, {
        auth,
        extras,
        body: { email: `inv-op-${randomUUID().slice(0, 12)}@inv.test`, role: "viewer" },
      }),
    ),
    op("listInvitations", false, "workspace", (auth, t, extras) =>
      request("GET", `${ADMIN}/workspaces/${t.workspaceId}/invitations`, { auth, extras }),
    ),
    // 対象は存在しない招待（舞台の招待を壊さないため）
    op("revokeInvitation", true, "workspace", (auth, t, extras) =>
      request("DELETE", `${ADMIN}/workspaces/${t.workspaceId}/invitations/${randomUUID()}`, { auth, extras }),
    ),
    // トークンは誤った値にする
    op("acceptInvitation", true, "self", (auth, _t, extras) =>
      request("POST", `${ADMIN}/me/invitations/accept`, { auth, extras, body: { token: `wrong-${randomUUID()}` } }),
    ),
    op("issuePasswordReset", true, "workspace", (auth, t, extras) =>
      request("POST", `${ADMIN}/workspaces/${t.workspaceId}/members/${t.userId}/password-reset`, { auth, extras }),
    ),
    op("issueMfaReset", true, "workspace", (auth, t, extras) =>
      request("POST", `${ADMIN}/workspaces/${t.workspaceId}/members/${t.userId}/mfa-reset`, { auth, extras }),
    ),
    op("listPersonalTokens", false, "self", (auth, _t, extras) => request("GET", `${ADMIN}/me/tokens`, { auth, extras })),
    op("createPersonalToken", true, "self", (auth, t, extras) =>
      request("POST", `${ADMIN}/me/tokens`, { auth, extras, body: { scopes: ["read"], workspaceIds: [t.workspaceId] } }),
    ),
    // 対象は存在しないトークン（舞台のトークンを壊さないため）
    op("revokePersonalToken", true, "self", (auth, _t, extras) => request("DELETE", `${ADMIN}/me/tokens/${randomUUID()}`, { auth, extras })),
    op("listApiKeys", false, "workspace", (auth, t, extras) => request("GET", `${ADMIN}/workspaces/${t.workspaceId}/api-keys`, { auth, extras })),
    op("createApiKey", true, "workspace", (auth, t, extras) =>
      request("POST", `${ADMIN}/workspaces/${t.workspaceId}/api-keys`, { auth, extras, body: { scopes: ["read"] } }),
    ),
    op("revokeApiKey", true, "workspace", (auth, t, extras) =>
      request("DELETE", `${ADMIN}/workspaces/${t.workspaceId}/api-keys/${t.apiKeyId}`, { auth, extras }),
    ),
    op("listAuditLog", false, "workspace", (auth, t, extras) => auditPageRaw(auth, t.workspaceId, undefined, extras)),
    op("listInstanceAuditLog", false, "instance", (auth, _t, extras) => auditPageRaw(auth, null, undefined, extras)),
    op("listWebhooks", false, "workspace", (auth, t, extras) => request("GET", webhooksPath(t.workspaceId), { auth, extras })),
    op("createWebhook", true, "workspace", (auth, t, extras) => createWebhookRaw(auth, t.workspaceId, OPERATION_WEBHOOK_URL, extras)),
    op("testWebhook", true, "workspace", async (auth, t, extras) =>
      testWebhookRaw(auth, t.workspaceId, await firstWebhookId(auth, t.workspaceId, extras), extras),
    ),
    op("listWebhookDeliveries", false, "workspace", async (auth, t, extras) =>
      request("GET", `${webhooksPath(t.workspaceId)}/${await firstWebhookId(auth, t.workspaceId, extras)}/deliveries`, { auth, extras }),
    ),
    op("rotateWebhookSecret", true, "workspace", async (auth, t, extras) =>
      request("POST", `${webhooksPath(t.workspaceId)}/${await firstWebhookId(auth, t.workspaceId, extras)}/rotate-secret`, { auth, extras }),
    ),
    // 対象は存在しない送信先（舞台の送信先を壊さないため）
    op("deleteWebhook", true, "workspace", (auth, t, extras) => request("DELETE", `${webhooksPath(t.workspaceId)}/${randomUUID()}`, { auth, extras })),
    op("removeMember", true, "workspace", (auth, t, extras) =>
      request("DELETE", `${ADMIN}/workspaces/${t.workspaceId}/members/${t.userId}`, { auth, extras }),
    ),
  ];

  function inviteRaw(adminSession: Session, workspaceId: string, email: string, role: WorkspaceRole): Promise<HttpResponse> {
    return request("POST", `${ADMIN}/workspaces/${workspaceId}/invitations`, { auth: session(adminSession), body: { email, role } });
  }

  function acceptInviteRaw(token: string, as: { newPassword: string } | { session: Session }): Promise<HttpResponse> {
    if ("session" in as) {
      return request("POST", `${ADMIN}/me/invitations/accept`, { auth: session(as.session), body: { token } });
    }
    return request("POST", `${ADMIN}/invitations/accept`, { body: { token, newPassword: as.newPassword } });
  }

  /** 招待し、トークンを受け取る。SMTPがなければ応答のリンクから、あればメールから取り出す。 */
  async function inviteAndGetToken(adminSession: Session, workspaceId: string, email: string, role: WorkspaceRole): Promise<string> {
    // メールで届く場合に備え、招待の前に届いているメールを覚えておく（メールの受け口がない環境では null）
    const seen = await mailCursor(email).catch(() => null);
    const res = ensure("招待", await inviteRaw(adminSession, workspaceId, email, role));
    const { inviteUrl } = JSON.parse(res.text) as { inviteUrl?: unknown };
    if (typeof inviteUrl === "string") {
      const token = new URL(inviteUrl).hash.match(/token=([^&]+)/)?.[1];
      if (!token) throw new SetupError("招待のリンクの読み取り", res);
      return token;
    }
    if (seen === null) throw new SetupError("招待のメールの受け取り（メールの受け口が使えない）", res);
    return waitForToken(email, seen, "invite");
  }

  const implemented = {
    baseUrl: base,
    adminOperations,
    login,
    loginRaw,
    loginPasswordOnly,
    async logout(s: Session): Promise<void> {
      ensure("ログアウト", await request("DELETE", `${ADMIN}/session`, { auth: session(s) }));
    },
    reauthenticate,
    reauthenticateRaw,
    nextTotpCode,
    meRaw,
    createEntryRaw: (auth: Auth, workspaceId: string, input: EntryInput) => createEntryRaw(auth, workspaceId, input),
    async createEntry(s: Session, workspaceId: string, input: EntryInput): Promise<{ id: string }> {
      const res = ensure("記事の作成", await createEntryRaw(session(s), workspaceId, input));
      const { id } = JSON.parse(res.text) as { id?: unknown };
      if (typeof id !== "string") throw new SetupError("記事の作成の応答の読み取り", res);
      return { id };
    },
    getEntryRaw: (auth: Auth, workspaceId: string, entryId: string) => getEntryRaw(auth, workspaceId, entryId),
    async getDraft(s: Session, workspaceId: string, entryId: string): Promise<Draft> {
      return toDraft(ensure("下書きの取得", await getEntryRaw(session(s), workspaceId, entryId)));
    },
    async listEntries(s: Session, workspaceId: string): Promise<{ id: string; slug: string; locale: string }[]> {
      const res = ensure("記事の一覧", await request("GET", entriesPath(workspaceId), { auth: session(s) }));
      const list = JSON.parse(res.text) as { id: string; locale: string; fields: Record<string, unknown> }[];
      return list.map((e) => ({ id: e.id, locale: e.locale, slug: String(e.fields["slug"] ?? "") }));
    },
    saveDraftRaw: (auth: Auth, workspaceId: string, entryId: string, changes: Partial<EntryInput>, revision: string, options?: { lockId?: string }) =>
      saveDraftRaw(auth, workspaceId, entryId, changes, revision, undefined, options?.lockId),
    acquireEditLockRaw: (auth: Auth, workspaceId: string, entryId: string) => request("POST", lockPath(workspaceId, entryId), { auth }),
    async acquireEditLock(s: Session, workspaceId: string, entryId: string): Promise<string> {
      return lockIdOf("編集のロック", await request("POST", lockPath(workspaceId, entryId), { auth: session(s) }));
    },
    takeOverEditLockRaw: (auth: Auth, workspaceId: string, entryId: string) => request("POST", `${lockPath(workspaceId, entryId)}/take-over`, { auth }),
    async takeOverEditLock(s: Session, workspaceId: string, entryId: string): Promise<string> {
      return lockIdOf("編集の引き継ぎ", await request("POST", `${lockPath(workspaceId, entryId)}/take-over`, { auth: session(s) }));
    },
    async releaseEditLock(s: Session, workspaceId: string, entryId: string, lockId: string): Promise<void> {
      ensure("編集のロックを外す", await request("DELETE", `${lockPath(workspaceId, entryId)}/${lockId}`, { auth: session(s) }));
    },
    async saveDraft(s: Session, workspaceId: string, entryId: string, changes: Partial<EntryInput>): Promise<void> {
      const current = toDraft(ensure("下書きの取得", await getEntryRaw(session(s), workspaceId, entryId)));
      ensure("下書きの保存", await saveDraftRaw(session(s), workspaceId, entryId, changes, current.revision));
    },
    publishRaw: (auth: Auth, workspaceId: string, entryId: string, options?: { correction?: boolean; note?: string }) => publishRaw(auth, workspaceId, entryId, options),
    scheduleRaw: (auth: Auth, workspaceId: string, entryId: string, at: Date, options?: { correction?: boolean; note?: string }) =>
      scheduleRaw(auth, workspaceId, entryId, at, options),
    async schedulePublish(s: Session, workspaceId: string, entryId: string, at: Date): Promise<void> {
      ensure("予約公開", await scheduleRaw(session(s), workspaceId, entryId, at));
    },
    async cancelSchedule(s: Session, workspaceId: string, entryId: string): Promise<void> {
      ensure("予約の取り消し", await request("DELETE", `${entriesPath(workspaceId)}/${entryId}/schedule`, { auth: session(s) }));
    },
    async publishEntry(s: Session, workspaceId: string, entryId: string, options?: { correction?: boolean; note?: string }): Promise<void> {
      ensure(options?.correction ? "訂正としての公開" : "公開", await publishRaw(session(s), workspaceId, entryId, options));
    },
    async editCorrectionNote(s: Session, workspaceId: string, entryId: string, versionId: string, note: string): Promise<void> {
      ensure("訂正の説明の書き直し", await editCorrectionNoteRaw(session(s), workspaceId, entryId, versionId, note));
    },
    async markVerified(s: Session, workspaceId: string, entryId: string, memo: string): Promise<void> {
      ensure("確認済みの記録", await markVerifiedRaw(session(s), workspaceId, entryId, memo));
    },
    async unpublishEntry(s: Session, workspaceId: string, entryId: string): Promise<void> {
      ensure("非公開", await unpublishRaw(session(s), workspaceId, entryId));
    },
    async deleteEntry(s: Session, workspaceId: string, entryId: string): Promise<void> {
      ensure("記事の削除", await deleteEntryRaw(session(s), workspaceId, entryId));
    },
    async changePublishSettings(s: Session, workspaceId: string, entryId: string): Promise<void> {
      ensure("公開設定の変更", await togglePublishSettingsRaw(session(s), workspaceId, entryId));
    },
    async setIncludeInAiMaterials(s: Session, workspaceId: string, entryId: string, include: boolean): Promise<void> {
      ensure("公開設定の変更", await request("PATCH", settingsPath(workspaceId, entryId), { auth: session(s), body: { includeInAiMaterials: include } }));
    },
    async listVersions(s: Session, workspaceId: string, entryId: string): Promise<VersionInfo[]> {
      const res = ensure("版の一覧", await versionsRaw(session(s), workspaceId, entryId));
      const list = JSON.parse(res.text) as { id?: unknown; isCorrection?: unknown; erased?: unknown }[];
      return list.map((v) => {
        if (typeof v.id !== "string" || typeof v.isCorrection !== "boolean" || typeof v.erased !== "boolean") throw new SetupError("版の一覧の読み取り", res);
        return { id: v.id, isCorrection: v.isCorrection, erased: v.erased };
      });
    },
    getVersionRaw: (auth: Auth, workspaceId: string, entryId: string, versionId: string) => versionRaw(auth, workspaceId, entryId, versionId),
    async createTranslation(s: Session, workspaceId: string, sourceEntryId: string, locale: string): Promise<{ id: string }> {
      const res = ensure("翻訳の作成", await createTranslationRaw(session(s), workspaceId, sourceEntryId, locale));
      const { id } = JSON.parse(res.text) as { id?: unknown };
      if (typeof id !== "string") throw new SetupError("翻訳の作成の応答の読み取り", res);
      return { id };
    },
    async getTranslationInfo(s: Session, workspaceId: string, entryId: string): Promise<TranslationInfo> {
      const res = ensure("翻訳の情報の取得", await translationInfoRaw(session(s), workspaceId, entryId));
      const t = JSON.parse(res.text) as {
        groupId?: unknown;
        locale?: unknown;
        sourceEntryId?: unknown;
        reflectedSourceVersionId?: unknown;
        sourceUpdate?: { corrected?: unknown } | null;
      };
      const idOrNull = (v: unknown) => (typeof v === "string" ? v : v === null ? null : undefined);
      const sourceEntryId = idOrNull(t.sourceEntryId);
      const reflectedSourceVersionId = idOrNull(t.reflectedSourceVersionId);
      if (typeof t.groupId !== "string" || typeof t.locale !== "string" || sourceEntryId === undefined || reflectedSourceVersionId === undefined) {
        throw new SetupError("翻訳の情報の読み取り", res);
      }
      // sourceUpdate: 更新がなければ null、あれば { corrected }
      if (t.sourceUpdate !== null && (typeof t.sourceUpdate !== "object" || typeof t.sourceUpdate?.corrected !== "boolean")) {
        throw new SetupError("翻訳の情報の読み取り（原文の更新）", res);
      }
      return {
        groupId: t.groupId,
        locale: t.locale,
        sourceEntryId,
        reflectedSourceVersionId,
        sourceUpdated: t.sourceUpdate !== null,
        sourceCorrected: t.sourceUpdate?.corrected === true,
      };
    },
    async getSourceDiff(s: Session, workspaceId: string, entryId: string): Promise<SourceDiff> {
      const res = ensure("原文の差分の取得", await sourceDiffRaw(session(s), workspaceId, entryId));
      const d = JSON.parse(res.text) as {
        fromVersionId?: string | null;
        toVersionId?: string | null;
        diff?: { changedFields?: string[]; sections?: { anchor?: string | null; kind?: string; before?: string | null; after?: string | null }[] } | null;
      };
      if (d.fromVersionId === undefined || d.toVersionId === undefined || d.diff === undefined) throw new SetupError("原文の差分の読み取り", res);
      if (d.diff === null) return { fromVersionId: d.fromVersionId, toVersionId: d.toVersionId, available: false, changedFields: [], sections: [] };
      if (!Array.isArray(d.diff.changedFields) || !Array.isArray(d.diff.sections)) throw new SetupError("原文の差分の読み取り", res);
      return {
        fromVersionId: d.fromVersionId,
        toVersionId: d.toVersionId,
        available: true,
        changedFields: d.diff.changedFields,
        sections: d.diff.sections.map((x) => ({
          anchor: x.anchor ?? null,
          kind: x.kind as SourceDiff["sections"][number]["kind"],
          before: x.before ?? null,
          after: x.after ?? null,
        })),
      };
    },
    async requestReview(s: Session, workspaceId: string, entryId: string): Promise<void> {
      ensure("レビューの依頼", await requestReviewRaw(session(s), workspaceId, entryId));
    },
    async getTodo(s: Session, workspaceId: string): Promise<TodoItem[]> {
      const res = ensure("やることの取得", await todoRaw(session(s), workspaceId));
      const body = JSON.parse(res.text) as {
        items?: { kind?: unknown; entry?: { id?: unknown } | null; notification?: { action?: unknown; targetIds?: unknown } }[];
      };
      if (!Array.isArray(body.items)) throw new SetupError("やることの読み取り", res);
      return body.items.map((i) => {
        if (typeof i.kind !== "string") throw new SetupError("やることの項目の読み取り", res);
        if (i.kind === "notification") {
          const n = i.notification;
          if (typeof n?.action !== "string" || !Array.isArray(n.targetIds)) throw new SetupError("やることの通知の読み取り", res);
          return { kind: "notification", entryId: null, notification: { action: n.action, targetIds: n.targetIds.map(String) } };
        }
        if (typeof i.entry?.id !== "string") throw new SetupError("やることの項目の読み取り", res);
        return { kind: i.kind as TodoItem["kind"], entryId: i.entry.id };
      });
    },
    markSourceReflectedRaw: (auth: Auth, workspaceId: string, entryId: string) => markSourceReflectedRaw(auth, workspaceId, entryId),
    async markSourceReflected(s: Session, workspaceId: string, entryId: string): Promise<void> {
      ensure("原文の更新の反映済み", await markSourceReflectedRaw(session(s), workspaceId, entryId));
    },
    uploadMediaRaw: (s: Session, workspaceId: string, file: UploadFile) => uploadMediaRaw(session(s), workspaceId, file),
    deleteMediaRaw: (s: Session, workspaceId: string, mediaId: string, options?: { force?: boolean }) =>
      request("DELETE", `${mediaPath(workspaceId)}/${mediaId}${options?.force ? "?force=true" : ""}`, { auth: session(s) }),
    async uploadMedia(s: Session, workspaceId: string, file: UploadFile): Promise<{ id: string; url: string }> {
      const res = ensure("メディアのアップロード", await uploadMediaRaw(session(s), workspaceId, file));
      const { id, url } = JSON.parse(res.text) as { id?: unknown; url?: unknown };
      if (typeof id !== "string" || typeof url !== "string") throw new SetupError("メディアのアップロードの応答の読み取り", res);
      return { id, url };
    },
    async fetchMediaRaw(url: string): Promise<HttpResponse & { bytes: Buffer }> {
      // 配信のURLは公開URL（テストの環境では待ち受けていない）を指すので、パスだけを使ってこのサーバーに送る
      const u = new URL(url);
      const res = await fetch(new URL(u.pathname + u.search, base), { redirect: "manual" });
      const bytes = Buffer.from(await res.arrayBuffer());
      return { status: res.status, headers: res.headers, setCookies: res.headers.getSetCookie(), text: bytes.toString("latin1"), bytes };
    },
    setEmbeddingBudgetRaw(auth: Auth, workspaceId: string, budget: EmbeddingBudget): Promise<HttpResponse> {
      return setEmbeddingBudgetRaw(auth, workspaceId, budget);
    },
    setAssistBudgetRaw: (auth: Auth, workspaceId: string, budget: AssistBudget) => setAssistBudgetRaw(auth, workspaceId, budget),
    async setAssistSettings(s: Session, workspaceId: string, settings: AssistSettings): Promise<void> {
      ensure("編集者向けのAI支援の設定の変更", await setAssistSettingsRaw(session(s), workspaceId, settings));
    },
    setAssistSettingsRaw: (auth: Auth, workspaceId: string, settings: AssistSettings) => setAssistSettingsRaw(auth, workspaceId, settings),
    startConflictCheckRaw: (auth: Auth, workspaceId: string, entryId: string) => request("POST", conflictsPath(workspaceId, entryId), { auth }),
    async startConflictCheck(s: Session, workspaceId: string, entryId: string): Promise<void> {
      ensure("矛盾の検知の開始", await request("POST", conflictsPath(workspaceId, entryId), { auth: session(s) }));
    },
    async conflictCheck(s: Session, workspaceId: string, entryId: string): Promise<ConflictCheck> {
      const res = ensure("矛盾の検知の結果の読み取り", await request("GET", conflictsPath(workspaceId, entryId), { auth: session(s) }));
      const r = JSON.parse(res.text) as Partial<ConflictCheck>;
      if (typeof r.state !== "string" || !Array.isArray(r.conflicts)) throw new SetupError("矛盾の検知の結果の読み取り", res);
      return r as ConflictCheck;
    },
    async embeddingSwitch(s: Session): Promise<EmbeddingSwitchStatus> {
      const res = ensure("埋め込みモデルの切り替えの状態", await request("GET", `${ADMIN}/embedding-switch`, { auth: session(s) }));
      return JSON.parse(res.text) as EmbeddingSwitchStatus;
    },
    startEmbeddingSwitchRaw(auth: Auth): Promise<HttpResponse> {
      return request("POST", `${ADMIN}/embedding-switch/start`, { auth });
    },
    async setWorkspaceLimits(s: Session, workspaceId: string, limits: WorkspaceLimits): Promise<void> {
      ensure("ワークスペースの上限の変更", await setWorkspaceLimitsRaw(session(s), workspaceId, limits));
    },
    async setAnswerSettings(s: Session, workspaceId: string, settings: AnswerSettings): Promise<void> {
      ensure("回答の設定の変更", await setAnswerSettingsRaw(session(s), workspaceId, settings));
    },
    setAnswerSettingsRaw: (auth: Auth, workspaceId: string, settings: AnswerSettings) => setAnswerSettingsRaw(auth, workspaceId, settings),
    setWorkspaceLlmRaw: (auth: Auth, workspaceId: string, body: WorkspaceLlm | Record<string, unknown>) =>
      request("PUT", `${ADMIN}/workspaces/${workspaceId}/llm`, { auth, body }),
    getWorkspaceLlmRaw: (auth: Auth, workspaceId: string) => request("GET", `${ADMIN}/workspaces/${workspaceId}/llm`, { auth }),
    startWorkspaceEmbeddingSwitchRaw: (auth: Auth, workspaceId: string) =>
      request("POST", `${ADMIN}/workspaces/${workspaceId}/llm/embedding-switch`, { auth }),
    async setQuestionLogSettings(s: Session, workspaceId: string, settings: QuestionLogSettings): Promise<void> {
      ensure("質問ログの設定の変更", await setQuestionLogSettingsRaw(session(s), workspaceId, settings));
    },
    setQuestionLogSettingsRaw: (auth: Auth, workspaceId: string, settings: QuestionLogSettings) => setQuestionLogSettingsRaw(auth, workspaceId, settings),
    async questionCandidates(s: Session, workspaceId: string): Promise<QuestionCandidate[]> {
      const res = ensure("質問ログの読み取り", await questionLogRaw(session(s), workspaceId));
      const { candidates } = JSON.parse(res.text) as { candidates?: unknown };
      if (!Array.isArray(candidates)) throw new SetupError("質問ログの候補の読み取り", res);
      return candidates as QuestionCandidate[];
    },
    questionLogRaw: (auth: Auth, workspaceId: string) => questionLogRaw(auth, workspaceId),
    async eraseCandidates(s: Session, workspaceId: string, entryId: string, versionId: string): Promise<string[]> {
      const res = ensure("抹消の候補の取得", await eraseCandidatesRaw(session(s), workspaceId, entryId, versionId));
      const { media } = JSON.parse(res.text) as { media?: unknown };
      if (!Array.isArray(media) || !media.every((m) => typeof m === "string")) throw new SetupError("抹消の候補の読み取り", res);
      return media as string[];
    },
    eraseVersionRaw: (s: Session, workspaceId: string, entryId: string, versionId: string, deleteMediaIds: string[]) =>
      eraseVersionRaw(session(s), workspaceId, entryId, versionId, deleteMediaIds),
    registerQueryRaw,
    async registerQuery(workspaceId: string, registerKey: string, spec: QuerySpec): Promise<RegisteredQuery> {
      const res = ensure("クエリの登録", await registerQueryRaw(workspaceId, registerKey, spec));
      const { id } = JSON.parse(res.text) as { id?: unknown };
      if (typeof id !== "string") throw new SetupError("クエリの登録の応答の読み取り", res);
      return { workspaceId, ref: id };
    },
    deliveryRaw(
      workspaceIdInUrl: string,
      apiKey: string | null,
      query: RegisteredQuery,
      variables: Record<string, unknown> = {},
      extras?: RequestExtras,
    ): Promise<HttpResponse> {
      return request("GET", `/ws/${workspaceIdInUrl}/graphql`, {
        auth: deliveryAuth(apiKey),
        extras: { ...extras, query: { ...(extras?.query ?? {}), id: query.ref, variables: JSON.stringify(variables) } },
      });
    },
    async search(workspaceId: string, apiKey: string, query: RegisteredQuery, input: { text: string; locale?: string; first?: number; mode?: SearchMode }): Promise<SearchResult> {
      // 検索の方法は GraphQL の列挙の値（HYBRID・VECTOR・KEYWORD）で渡す
      const { mode, ...rest } = input;
      const variables = mode === undefined ? rest : { ...rest, mode: mode.toUpperCase() };
      // 検索は POST だけ（検索語を URL に載せない）
      const res = ensure(
        "サイト内検索",
        await request("POST", `/ws/${workspaceId}/graphql`, {
          auth: deliveryAuth(apiKey),
          body: { id: query.ref, variables },
        }),
      );
      const body = JSON.parse(res.text) as { data?: { search?: Record<string, unknown> }; errors?: unknown };
      const r = body.data?.search;
      if (!r || body.errors) throw new SetupError("サイト内検索の応答の読み取り", res);
      // GraphQL の列挙の値（AVAILABLE・BUDGET_EXHAUSTED・DISABLED）を、ドライバーの値にする
      const VALUES: Record<string, string> = { AVAILABLE: "available", BUDGET_EXHAUSTED: "budgetExhausted", DISABLED: "disabled" };
      return {
        hits: r["hits"] as SearchHit[],
        vectorSearch: VALUES[String(r["vectorSearch"])] as SearchResult["vectorSearch"],
        keywordSearch: VALUES[String(r["keywordSearch"])] as SearchResult["keywordSearch"],
      };
    },
    async relatedEntries(workspaceId: string, apiKey: string, query: RegisteredQuery, input: { id: string; first?: number }): Promise<RelatedEntry[]> {
      const res = ensure(
        "関連記事",
        await request("GET", `/ws/${workspaceId}/graphql`, {
          auth: deliveryAuth(apiKey),
          extras: { query: { id: query.ref, variables: JSON.stringify(input) } },
        }),
      );
      const body = JSON.parse(res.text) as { data?: Record<string, { relatedEntries?: RelatedEntry[] } | null>; errors?: unknown };
      const entry = body.data?.[SINGLE];
      if (!entry || !entry.relatedEntries || body.errors) throw new SetupError("関連記事の応答の読み取り", res);
      return entry.relatedEntries;
    },
    answerRaw(workspaceIdInUrl: string, apiKey: string, query: RegisteredQuery, input: { question: string; locale?: string; visitorId?: string }) {
      return request("POST", `/ws/${workspaceIdInUrl}/graphql`, {
        auth: deliveryAuth(apiKey),
        body: { id: query.ref, variables: input },
        extras: { headers: { Accept: "text/event-stream, application/json" } },
      });
    },
    parseAnswer(res) {
      if (isEventStream(res)) return null;
      const body = JSON.parse(res.text) as { data?: { answer?: Record<string, unknown> | null } | null; errors?: unknown };
      if (body.errors || !body.data?.answer) return null;
      return toAnswerResult(body.data.answer);
    },
    parseAnswerStream(res) {
      if (!isEventStream(res)) return [];
      const out: AnswerEvent[] = [];
      for (const { event, data } of sseEvents(res.text)) {
        if (event !== "next") continue;
        const e = (data as { data?: { answerStream?: Record<string, unknown> | null } | null } | null)?.data?.answerStream;
        if (!e) continue;
        switch (e["__typename"]) {
          case "YomiAnswerVerdict":
            out.push({ type: "verdict", answerable: e["answerable"] as boolean, sources: e["sources"] as AnswerSource[] });
            break;
          case "YomiAnswerDelta":
            out.push({ type: "delta", text: e["text"] as string });
            break;
          case "YomiAnswerFinal":
            out.push({ type: "final", ...toAnswerResult(e) });
            break;
          default:
            throw new SetupError(`回答のイベントの種類 ${String(e["__typename"])} の読み取り`, res);
        }
      }
      return out;
    },
    answerErrors(res) {
      if (!isEventStream(res)) {
        try {
          const body = JSON.parse(res.text) as unknown;
          // HTTPのエラーの応答（回数制限など）は { error, message }
          const error = (body as { error?: unknown } | null)?.error;
          if (typeof error === "string") return [{ code: error, message: String((body as { message?: unknown }).message ?? "") }];
          return graphqlErrors(body);
        } catch {
          return [{ code: null, message: `HTTP ${res.status}` }];
        }
      }
      return sseEvents(res.text)
        .filter((e) => e.event === "next")
        .flatMap((e) => graphqlErrors(e.data));
    },
    deliveryPreviewRaw(
      workspaceIdInUrl: string,
      apiKey: string | null,
      previewToken: string,
      query: RegisteredQuery,
      variables: Record<string, unknown> = {},
    ): Promise<HttpResponse> {
      return request("GET", `/ws/${workspaceIdInUrl}/graphql`, {
        auth: deliveryAuth(apiKey),
        extras: { headers: { [PREVIEW_HEADER]: previewToken }, query: { id: query.ref, variables: JSON.stringify(variables) } },
      });
    },
    async issuePreviewToken(s: Session, workspaceId: string, entryId: string, options?: { ttlSeconds?: number }): Promise<{ id: string; token: string }> {
      const res = ensure("プレビュー用トークンの発行", await issuePreviewTokenRaw(session(s), workspaceId, entryId, options?.ttlSeconds));
      const { id, token } = JSON.parse(res.text) as { id?: unknown; token?: unknown };
      if (typeof id !== "string" || typeof token !== "string") throw new SetupError("プレビュー用トークンの読み取り", res);
      return { id, token };
    },
    async revokePreviewToken(s: Session, workspaceId: string, entryId: string, tokenId: string): Promise<void> {
      ensure("プレビュー用トークンの失効", await revokePreviewTokenRaw(session(s), workspaceId, entryId, tokenId));
    },
    deliveryAdHocRaw(workspaceIdInUrl: string, apiKey: string, spec: QuerySpec, variables: Record<string, unknown> = {}): Promise<HttpResponse> {
      return request("POST", `/ws/${workspaceIdInUrl}/graphql`, { auth: deliveryAuth(apiKey), body: { query: queryText(spec), variables } });
    },
    deliveryBatchRaw(workspaceIdInUrl: string, apiKey: string, queries: RegisteredQuery[]): Promise<HttpResponse> {
      return request("POST", `/ws/${workspaceIdInUrl}/graphql`, { auth: deliveryAuth(apiKey), body: queries.map((q) => ({ id: q.ref })) });
    },
    async getUrlHistory(s: Session, workspaceId: string, entryId: string): Promise<UrlChange[]> {
      const res = ensure("URLの変更履歴", await urlHistoryRaw(session(s), workspaceId, entryId));
      const list = JSON.parse(res.text) as { from?: unknown; to?: unknown }[];
      return list.map((c) => {
        if (typeof c.from !== "string" || typeof c.to !== "string") throw new SetupError("URLの変更履歴の読み取り", res);
        return { from: c.from, to: c.to };
      });
    },
    schemaInfoRaw: (auth: Auth, workspaceId: string) => schemaInfoRaw(auth, workspaceId),
    deployTemplateRaw: (auth: Auth, template: TemplateSpec, options?: { allowDestructive?: boolean; allowBrokenQueries?: boolean }) =>
      deployTemplateRaw(auth, template, options),
    async deployToken(s: Session, creds: Credentials): Promise<string> {
      // デプロイの範囲のトークンは、多要素認証で再認証してから発行する
      await reauthenticate(s, creds);
      const res = ensure(
        "デプロイの範囲の個人トークンの発行",
        await request("POST", `${ADMIN}/me/tokens`, { auth: session(s), body: { name: "inv-deploy", scopes: ["deploy"], workspaceIds: [] } }),
      );
      const { token } = JSON.parse(res.text) as { token?: unknown };
      if (typeof token !== "string") throw new SetupError("デプロイの範囲の個人トークンの読み取り", res);
      return token;
    },
    async workspaceTemplate(s: Session, workspaceId: string): Promise<string> {
      const res = ensure("スキーマ情報の取得", await schemaInfoRaw(session(s), workspaceId));
      const { template } = JSON.parse(res.text) as { template?: unknown };
      if (typeof template !== "string") throw new SetupError("テンプレートの名前の読み取り", res);
      return template;
    },
    switchTemplateRaw: (auth: Auth, workspaceId: string, options: { template: string; allowDestructive?: boolean; dryRun?: boolean; mapping?: TemplateMapping }) =>
      request("POST", `${ADMIN}/workspaces/${workspaceId}/template`, { auth, body: options }),
    async migrationState(s: Session, workspaceId: string): Promise<"ok" | "pending"> {
      const res = ensure("スキーマ情報の取得", await schemaInfoRaw(session(s), workspaceId));
      const { state } = JSON.parse(res.text) as { state?: unknown };
      if (state !== "ok" && state !== "pending") throw new SetupError("移行の状態の読み取り", res);
      return state;
    },
    loginHistoryRaw: (s: Session) => loginHistoryRaw(session(s)),
    async me(s: Session): Promise<{ userId: string; email: string }> {
      const res = ensure("本人の情報の取得", await meRaw(session(s)));
      const parsed = JSON.parse(res.text) as { id?: unknown; email?: unknown };
      if (typeof parsed.id !== "string" || typeof parsed.email !== "string") throw new SetupError("本人の情報の読み取り", res);
      return { userId: parsed.id, email: parsed.email };
    },
    async changePassword(s: Session, creds: Credentials, newPassword: string): Promise<void> {
      await reauthenticate(s, creds);
      ensure("パスワードの変更", await changePasswordRaw(s, creds.password, newPassword));
    },
    changePasswordRaw,
    createWorkspaceRaw(auth: Auth, name: string): Promise<HttpResponse> {
      return request("POST", `${ADMIN}/workspaces`, { auth, body: { name } });
    },
    deleteWorkspaceRaw: (auth: Auth, workspaceId: string, options: { confirmName?: string }) =>
      request("DELETE", `${ADMIN}/workspaces/${workspaceId}`, { auth, body: options }),
    // テンプレートを指定しなければ、テスト環境でデプロイ済みの BASE_TEMPLATE を使う（README「テスト環境」）
    async createWorkspace(s: Session, name: string, options: { template?: string } = {}): Promise<{ id: string }> {
      const res = ensure(
        "ワークスペースの作成",
        await request("POST", `${ADMIN}/workspaces`, { auth: session(s), body: { name, template: options.template ?? BASE_TEMPLATE.name } }),
      );
      const { id } = JSON.parse(res.text) as { id?: unknown };
      if (typeof id !== "string") throw new SetupError("ワークスペースの作成の応答の読み取り", res);
      return { id };
    },
    async changeMemberRole(s: Session, workspaceId: string, userId: string, role: WorkspaceRole): Promise<void> {
      ensure(
        "ロールの変更",
        await request("PATCH", `${ADMIN}/workspaces/${workspaceId}/members/${userId}`, { auth: session(s), body: { role } }),
      );
    },
    // 重い権限は、所属者の1人の応答の permissions（ロールの既定と付与の合計）と grants（個別の付与）
    // （server/src/http/admin_workspaces.rs）
    async memberPermissions(s: Session, workspaceId: string, userId: string): Promise<HeavyPermission[]> {
      return memberPermissionList(s, workspaceId, userId, "permissions");
    },
    async memberGrants(s: Session, workspaceId: string, userId: string): Promise<HeavyPermission[]> {
      return memberPermissionList(s, workspaceId, userId, "grants");
    },
    setMemberGrantsRaw(auth: Auth, workspaceId: string, userId: string, grants: HeavyPermission[]): Promise<HttpResponse> {
      return request("PATCH", `${ADMIN}/workspaces/${workspaceId}/members/${userId}`, { auth, body: { grants } });
    },
    setRelatedRaw(auth: Auth, workspaceId: string, entryId: string, settings: RelatedSettings): Promise<HttpResponse> {
      return request("PUT", `${entriesPath(workspaceId)}/${entryId}/related`, { auth, body: settings });
    },
    recomputeRelatedRaw(auth: Auth, workspaceId: string): Promise<HttpResponse> {
      return request("POST", `${ADMIN}/workspaces/${workspaceId}/related/recompute`, { auth });
    },
    async translationGroupId(s: Session, workspaceId: string, entryId: string): Promise<string> {
      const res = ensure("記事の取得", await getEntryRaw(session(s), workspaceId, entryId));
      const { translationGroupId } = JSON.parse(res.text) as { translationGroupId?: unknown };
      if (typeof translationGroupId !== "string") throw new SetupError("記事の翻訳グループの読み取り", res);
      return translationGroupId;
    },
    setPublishSettingsRaw(
      auth: Auth,
      workspaceId: string,
      entryId: string,
      changes: { includeInAiMaterials?: boolean; optOutOfAiTraining?: boolean; useForChatbot?: boolean },
    ): Promise<HttpResponse> {
      return request("PATCH", settingsPath(workspaceId, entryId), { auth, body: changes });
    },
    markVerifiedRaw(auth: Auth, workspaceId: string, entryId: string, memo: string): Promise<HttpResponse> {
      return markVerifiedRaw(auth, workspaceId, entryId, memo);
    },
    async removeMember(s: Session, workspaceId: string, userId: string): Promise<void> {
      ensure("所属から外す", await request("DELETE", `${ADMIN}/workspaces/${workspaceId}/members/${userId}`, { auth: session(s) }));
    },
    inviteRaw,
    acceptInviteRaw,
    async createPersonalToken(
      s: Session,
      workspaceIds: string[],
      options: { scopes?: TokenScope[]; expiresAt?: Date } = {},
    ): Promise<IssuedSecret> {
      const res = ensure(
        "個人トークンの発行",
        await request("POST", `${ADMIN}/me/tokens`, {
          auth: session(s),
          body: { scopes: options.scopes ?? ["read", "write"], workspaceIds, expiresAt: options.expiresAt?.toISOString() },
        }),
      );
      const { id, token } = JSON.parse(res.text) as { id?: unknown; token?: unknown };
      if (typeof id !== "string" || typeof token !== "string") throw new SetupError("個人トークンの発行の応答の読み取り", res);
      return { id, secret: token, response: res };
    },
    async createApiKey(s: Session, workspaceId: string, scopes: ApiKeyScope[], options: { expiresAt?: Date } = {}): Promise<IssuedSecret> {
      const res = ensure(
        "配信APIのキーの発行",
        await request("POST", `${ADMIN}/workspaces/${workspaceId}/api-keys`, {
          auth: session(s),
          body: { scopes, expiresAt: options.expiresAt?.toISOString() },
        }),
      );
      const { id, key } = JSON.parse(res.text) as { id?: unknown; key?: unknown };
      if (typeof id !== "string" || typeof key !== "string") throw new SetupError("配信APIのキーの発行の応答の読み取り", res);
      return { id, secret: key, response: res };
    },
    async revokeApiKey(s: Session, workspaceId: string, keyId: string): Promise<void> {
      ensure("配信APIのキーの失効", await request("DELETE", `${ADMIN}/workspaces/${workspaceId}/api-keys/${keyId}`, { auth: session(s) }));
    },
    async revokePersonalToken(s: Session, tokenId: string): Promise<void> {
      ensure("個人トークンの失効", await request("DELETE", `${ADMIN}/me/tokens/${tokenId}`, { auth: session(s) }));
    },
    requestPasswordResetRaw(email: string, extras?: RequestExtras): Promise<HttpResponse> {
      return request("POST", `${ADMIN}/password-resets`, { body: { email }, extras });
    },
    completePasswordResetRaw(token: string, newPassword: string): Promise<HttpResponse> {
      return request("POST", `${ADMIN}/password-resets/complete`, { body: { token, newPassword } });
    },
    issuePasswordResetRaw(s: Session, workspaceId: string | null, userId: string): Promise<HttpResponse> {
      const path = workspaceId === null ? `${ADMIN}/users/${userId}/password-reset` : `${ADMIN}/workspaces/${workspaceId}/members/${userId}/password-reset`;
      return request("POST", path, { auth: session(s) });
    },
    issueMfaResetRaw(s: Session, workspaceId: string | null, userId: string): Promise<HttpResponse> {
      const path = workspaceId === null ? `${ADMIN}/users/${userId}/mfa-reset` : `${ADMIN}/workspaces/${workspaceId}/members/${userId}/mfa-reset`;
      return request("POST", path, { auth: session(s) });
    },
    async createMember(adminSession: Session, workspaceId: string, role: WorkspaceRole, options: { mfa?: boolean; email?: string } = {}): Promise<Member> {
      const email = options.email ?? `member-${randomUUID().slice(0, 12)}@inv.test`;
      const token = await inviteAndGetToken(adminSession, workspaceId, email, role);
      const password = `inv-${randomUUID()}`;
      const accepted = ensure("招待の承諾", await acceptInviteRaw(token, { newPassword: password }));
      const { userId } = JSON.parse(accepted.text) as { userId?: unknown };
      if (typeof userId !== "string") throw new SetupError("招待の承諾の応答の読み取り", accepted);
      const member: Member = { email, password, userId };
      if (options.mfa ?? role === "admin") {
        const session = await login(member);
        member.totpSecret = await implemented.enrollTotp(session, member);
      }
      return member;
    },
    // サーバー上のCLI（create-server-admin）で作る。パスワードはCLIが作って一度だけ表示する
    async createServerAdmin(options: { mfa: boolean }): Promise<Member> {
      const email = `inv-root-${randomUUID().slice(0, 12)}@inv.test`;
      const created = await runServerCli(validServerConfigFromEnv(), ["create-server-admin", "--email", email]);
      const password = created.output.match(/Password \(shown only once\): (\S+)/)?.[1];
      if (created.exitCode !== 0 || !password) throw new Error(`サーバー管理者を作れない:\n${created.output}`);
      const adminSession = await loginPasswordOnly({ email, password });
      const me = ensure("サーバー管理者の情報の取得", await meRaw(session(adminSession)));
      const { id } = JSON.parse(me.text) as { id?: unknown };
      if (typeof id !== "string") throw new SetupError("サーバー管理者の情報の読み取り", me);
      const member: Member = { email, password, userId: id };
      if (options.mfa) member.totpSecret = await implemented.enrollTotp(adminSession, member);
      return member;
    },
    // 管理画面は /admin/ の下（server/src/http/admin_ui.rs）
    adminUiRaw(path: string): Promise<HttpResponse> {
      return request("GET", `/admin${path.startsWith("/") ? path : `/${path}`}`);
    },
    corsProbeRaw(kind: "adminApi" | "deliveryApi", method: "GET" | "OPTIONS", origin: string, workspaceId: string): Promise<HttpResponse> {
      const path = kind === "adminApi" ? `${ADMIN}/me` : `/ws/${workspaceId}/graphql`;
      const headers: Record<string, string> = { Origin: origin };
      if (method === "OPTIONS") {
        headers["Access-Control-Request-Method"] = "POST";
        headers["Access-Control-Request-Headers"] = "authorization, content-type";
      }
      return request(method, path, { extras: { headers } });
    },
    async addExistingMember(adminSession: Session, workspaceId: string, member: Member, role: WorkspaceRole): Promise<void> {
      const token = await inviteAndGetToken(adminSession, workspaceId, member.email, role);
      const session = await login(member);
      ensure("既存の利用者による招待の承諾", await acceptInviteRaw(token, { session }));
    },
    // 新しい順に、続きがなくなるまでページをたどってすべて読む
    auditLogRaw: (auth: Auth, workspaceId: string | null, filter?: AuditFilter) => auditLogRaw(auth, workspaceId, filter),
    exportAuditLogRaw: (auth: Auth, workspaceId: string | null, filter?: AuditFilter) => exportAuditLogRaw(auth, workspaceId, filter),
    createAuditSinkRaw: (auth: Auth, workspaceId: string, url: string) => createAuditSinkRaw(auth, workspaceId, url),
    async createAuditSink(s: Session, workspaceId: string, url: string): Promise<{ id: string; secret: string }> {
      const res = ensure("監査ログの書き出し先の作成", await createAuditSinkRaw(session(s), workspaceId, url));
      const { id, secret } = JSON.parse(res.text) as { id?: unknown; secret?: unknown };
      if (typeof id !== "string" || typeof secret !== "string") throw new SetupError("監査ログの書き出し先の読み取り", res);
      return { id, secret };
    },
    async readAuditLog(s: Session, workspaceId: string | null): Promise<AuditEntry[]> {
      const out: AuditEntry[] = [];
      let before: string | undefined;
      for (;;) {
        const res = ensure("監査ログの読み取り", await auditPageRaw(session(s), workspaceId, before));
        const page = JSON.parse(res.text) as { entries?: unknown; nextBefore?: unknown };
        if (!Array.isArray(page.entries)) throw new SetupError("監査ログの応答の読み取り", res);
        for (const raw of page.entries as Record<string, unknown>[]) {
          const { id, action, actorUserId, targetIds } = raw;
          if (
            typeof id !== "string" ||
            typeof action !== "string" ||
            !(typeof actorUserId === "string" || actorUserId === null) ||
            !Array.isArray(targetIds) ||
            !targetIds.every((t) => typeof t === "string")
          ) {
            throw new SetupError("監査ログの記録の読み取り", res);
          }
          out.push({ id, action, actorUserId, targetIds: targetIds as string[], raw });
        }
        if (page.nextBefore === null || page.nextBefore === undefined) return out;
        if (typeof page.nextBefore !== "string") throw new SetupError("監査ログの続きの読み取り", res);
        before = page.nextBefore;
      }
    },
    async createWebhook(s: Session, workspaceId: string, url: string): Promise<{ id: string; secret: string }> {
      const res = ensure("Webhookの登録", await createWebhookRaw(session(s), workspaceId, url));
      const { id, secret } = JSON.parse(res.text) as { id?: unknown; secret?: unknown };
      if (typeof id !== "string" || typeof secret !== "string") throw new SetupError("Webhookの登録の応答の読み取り", res);
      return { id, secret };
    },
    testWebhookRaw: (s: Session, workspaceId: string, webhookId: string) => testWebhookRaw(session(s), workspaceId, webhookId),
    beginPasskeyRaw: (s: Session): Promise<HttpResponse> => request("POST", `${ADMIN}/me/mfa/passkeys`, { auth: session(s), body: { name: "inv test key" } }),
    async enrollPasskey(s: Session, creds: Credentials): Promise<PasskeyHandle> {
      const begin = () => request("POST", `${ADMIN}/me/mfa/passkeys`, { auth: session(s), body: { name: "inv test key" } });
      let started = await begin();
      // パスワードでログインしたセッションなら、再認証してからやり直す（SSOのセッションは、ログインの直後なら登録できる）
      if (started.status === 403 && started.text.includes("reauth_required")) {
        await reauthenticate(s, creds);
        started = await begin();
      }
      ensure("パスキーの登録の開始", started);
      const { credential, passkey } = createCredential(JSON.parse(started.text) as Parameters<typeof createCredential>[0], publicOrigin);
      ensure("パスキーの登録の確定", await request("POST", `${ADMIN}/me/mfa/passkeys/confirm`, { auth: session(s), body: { credential } }));
      softPasskeys.set(passkey.credentialId, passkey);
      return { credentialId: passkey.credentialId };
    },
    async reauthenticateWithPasskey(s: Session, handle: PasskeyHandle): Promise<void> {
      const passkey = softPasskeys.get(handle.credentialId);
      if (!passkey) throw new Error("このドライバーで登録していないパスキーです");
      const started = ensure("パスキーでの再認証の開始", await request("POST", `${ADMIN}/session/reauth/passkey`, { auth: session(s) }));
      const assertion = getAssertion(JSON.parse(started.text) as Parameters<typeof getAssertion>[0], passkey);
      ensure("パスキーでの再認証", await request("POST", `${ADMIN}/session/reauth`, { auth: session(s), body: { passkey: assertion } }));
    },
    loginWithOidcRaw,
    async linkOidcIdentity(s: Session): Promise<void> {
      const start = ensure("SSOのひも付けの開始", await request("POST", `${ADMIN}/me/identities/oidc/start`, { auth: session(s) }));
      // IdPからの戻り（別のサイトからの移動）では、SameSite=Strict のセッションのCookieは送られない
      const flowCookies = new Map<string, string>();
      applySetCookies(flowCookies, start.setCookies);
      const back = await viaIdp(start, flowCookies);
      const location = back.headers.get("location") ?? "";
      if (back.status !== 303 || location.includes("error=")) throw new SetupError(`SSOのひも付け（戻り先: ${location}）`, back);
    },
    async enrollTotp(s: Session, creds: Credentials): Promise<string> {
      await reauthenticate(s, creds);
      const setup = ensure("TOTPの設定の開始", await request("POST", `${ADMIN}/me/mfa/totp`, { auth: session(s) }));
      const { secret } = JSON.parse(setup.text) as { secret?: unknown };
      if (typeof secret !== "string") throw new SetupError("TOTPの設定の開始の応答の読み取り", setup);
      ensure(
        "TOTPの設定の確定",
        await request("POST", `${ADMIN}/me/mfa/totp/confirm`, { auth: session(s), body: { code: await nextTotpCode(secret) } }),
      );
      return secret;
    },
  } satisfies Partial<YomiClient>;

  // まだない操作は、呼ばれたときに NotImplementedError を投げる
  return new Proxy(implemented, {
    get(target, prop, receiver) {
      if (typeof prop !== "string" || prop in target) return Reflect.get(target, prop, receiver);
      if (prop === "then") return undefined; // Promise と取り違えられないように
      return () => todo(`YomiClient.${prop}`);
    },
  }) as unknown as YomiClient;
}
