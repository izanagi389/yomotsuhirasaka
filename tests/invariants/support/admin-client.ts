// ドライバーの一部: 管理APIへの接続（CmsClient）。管理APIのURLや本文の形は、ここに閉じ込める。
// 規則は driver.ts の冒頭のとおり（合否の判定を書かない、失敗を成功に見せかけない、Raw は応答をそのまま返す）。
//
// サーバーにまだない操作は、呼び出されたときに NotImplementedError を投げる。

import { randomUUID } from "node:crypto";
import type { AdminOperation, Auth, CmsClient, Credentials, LoginOptions, OperationTarget, RequestExtras, Session, WorkspaceRole } from "./driver.js";
import { isSuccess, parseSetCookie, toHttpResponse, type HttpResponse } from "./http.js";
import { totp } from "./totp.js";

/** セッションのCookieの名前以外の、管理APIの細部。 */
const ADMIN = "/api/admin";
const CSRF_HEADER = "X-CSRF-Token";

interface RequestOptions {
  auth?: Auth;
  body?: unknown;
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

export function connectAdmin(baseUrl: string, todo: (what: string) => never): CmsClient {
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
    let body: string | undefined;
    if (options.body !== undefined) {
      headers["Content-Type"] = "application/json";
      body = JSON.stringify(options.body);
    }
    Object.assign(headers, options.extras?.headers ?? {});
    const res = await fetch(url, { method, headers, body, redirect: "manual" });
    return toHttpResponse(res);
  }

  const session = (s: Session): Auth => ({ kind: "session", session: s, csrf: "valid" });

  /**
   * TOTPのコードを作る。サーバーは同じ時間の区切りのコードを二度受け付けないので、
   * 秘密ごとに使った区切りを覚え、前後1区切りの中でまだ使っていないものを使う（なければ次の区切りまで待つ）。
   */
  const usedSteps = new Map<string, number>();
  async function nextTotpCode(secret: string): Promise<string> {
    for (;;) {
      const current = Math.floor(Date.now() / 30_000);
      const last = usedSteps.get(secret) ?? -Infinity;
      const step = [current - 1, current, current + 1].find((s) => s > last);
      if (step !== undefined) {
        usedSteps.set(secret, step);
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

  async function reauthenticate(s: Session, creds: Credentials): Promise<void> {
    const method = creds.totpSecret ? { totpCode: await nextTotpCode(creds.totpSecret) } : { password: creds.password };
    ensure("再認証", await reauthenticateRaw(s, method));
  }

  function changePasswordRaw(s: Session, currentPassword: string, newPassword: string): Promise<HttpResponse> {
    return request("POST", `${ADMIN}/me/password`, { auth: session(s), body: { currentPassword, newPassword } });
  }

  const meRaw = (auth: Auth, extras?: RequestExtras): Promise<HttpResponse> => request("GET", `${ADMIN}/me`, { auth, extras });

  const op = (
    name: string,
    mutating: boolean,
    scope: AdminOperation["scope"],
    call: (auth: Auth, target: OperationTarget, extras?: RequestExtras) => Promise<HttpResponse>,
  ): AdminOperation => ({ name, mutating, scope, call });

  // 管理APIの操作の一覧（サーバーにある操作だけ。操作を作ったら、ここにも足す）
  const adminOperations: AdminOperation[] = [
    op("me", false, "self", (auth, _t, extras) => meRaw(auth, extras)),
    op("listMySessions", false, "self", (auth, _t, extras) => request("GET", `${ADMIN}/me/sessions`, { auth, extras })),
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
    op("removeMember", true, "workspace", (auth, t, extras) =>
      request("DELETE", `${ADMIN}/workspaces/${t.workspaceId}/members/${t.userId}`, { auth, extras }),
    ),
  ];

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
    meRaw,
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
    async createWorkspace(s: Session, name: string, options: { template?: string } = {}): Promise<{ id: string }> {
      const res = ensure(
        "ワークスペースの作成",
        await request("POST", `${ADMIN}/workspaces`, { auth: session(s), body: { name, template: options.template } }),
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
    async removeMember(s: Session, workspaceId: string, userId: string): Promise<void> {
      ensure("所属から外す", await request("DELETE", `${ADMIN}/workspaces/${workspaceId}/members/${userId}`, { auth: session(s) }));
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
  } satisfies Partial<CmsClient>;

  // まだない操作は、呼ばれたときに NotImplementedError を投げる
  return new Proxy(implemented, {
    get(target, prop, receiver) {
      if (typeof prop !== "string" || prop in target) return Reflect.get(target, prop, receiver);
      if (prop === "then") return undefined; // Promise と取り違えられないように
      return () => todo(`CmsClient.${prop}`);
    },
  }) as unknown as CmsClient;
}
