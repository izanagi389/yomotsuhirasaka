// 評価用の CMS の管理 API と配信 API の呼び出し。URL と本文の形は、約束のテストのドライバー
// （tests/invariants/support/admin-client.ts）と同じ。ドライバーはテスト環境の環境変数を前提にするので、
// 評価に要る操作だけをここに持つ。失敗はすべて例外にする（握りつぶさない）。

import { totp } from "../../../tests/invariants/support/totp.ts";

const ADMIN = "/api/admin";
const TOTP_STEP_MS = 30_000;

export interface Credentials {
  email: string;
  password: string;
  totpSecret?: string;
}

export type SearchMode = "hybrid" | "vector" | "keyword";

export interface SearchHit {
  entryId: string;
  anchor: string | null;
  breadcrumb: string[];
  score: number;
}

export interface SearchResult {
  hits: SearchHit[];
  vectorSearch: string;
  keywordSearch: string;
}

interface CmsResponse {
  status: number;
  text: string;
  setCookies: string[];
}

export class CmsError extends Error {
  readonly status: number;
  constructor(what: string, res: CmsResponse) {
    super(`${what} failed: status ${res.status}: ${res.text.slice(0, 300)}`);
    this.status = res.status;
  }
}

export class Cms {
  readonly #base: string;
  readonly #cookies = new Map<string, string>();
  #csrf = "";
  #lastTotpStep = -Infinity;
  #creds: Credentials | null = null;

  constructor(baseUrl: string) {
    this.#base = baseUrl.replace(/\/+$/, "");
  }

  async #request(method: string, path: string, options: { body?: unknown; bearer?: string } = {}): Promise<CmsResponse> {
    const headers: Record<string, string> = {};
    if (options.bearer) headers["Authorization"] = `Bearer ${options.bearer}`;
    else {
      if (this.#cookies.size > 0) headers["Cookie"] = [...this.#cookies].map(([k, v]) => `${k}=${v}`).join("; ");
      if (this.#csrf) headers["X-CSRF-Token"] = this.#csrf;
    }
    if (options.body !== undefined) headers["Content-Type"] = "application/json";
    const init: RequestInit = { method, headers, body: options.body === undefined ? undefined : JSON.stringify(options.body), redirect: "manual" };
    let res: globalThis.Response;
    try {
      res = await fetch(this.#base + path, init);
    } catch (error) {
      // 取り込みの CLI を待つ間に、サーバーが閉じた keep-alive の接続を使い回すことがあるので、1回だけやり直す
      // （やり直すのは管理の操作だけ。二重になっても、キーやトークンが1つ余分にできるだけ）
      if (!(error instanceof TypeError)) throw error;
      res = await fetch(this.#base + path, init);
    }
    const setCookies = res.headers.getSetCookie();
    for (const h of setCookies) {
      const [pair] = h.split(";");
      const eq = pair!.indexOf("=");
      const name = pair!.slice(0, eq).trim();
      const value = pair!.slice(eq + 1).trim();
      if (value === "" || /max-age=0/i.test(h)) this.#cookies.delete(name);
      else this.#cookies.set(name, value);
    }
    return { status: res.status, text: await res.text(), setCookies };
  }

  async #ok(what: string, method: string, path: string, options: { body?: unknown; bearer?: string } = {}): Promise<Record<string, unknown>> {
    const res = await this.#request(method, path, options);
    if (res.status < 200 || res.status >= 300) throw new CmsError(what, res);
    return res.text === "" ? {} : (JSON.parse(res.text) as Record<string, unknown>);
  }

  /** 同じ時間の区切りのコードは一度しか使えないので、使った区切りより後のコードを作る。 */
  async #nextTotp(secret: string): Promise<string> {
    for (;;) {
      const current = Math.floor(Date.now() / TOTP_STEP_MS);
      const step = [current, current + 1].find((s) => s > this.#lastTotpStep);
      if (step !== undefined) {
        this.#lastTotpStep = step;
        return totp(secret, new Date(step * TOTP_STEP_MS));
      }
      await new Promise((r) => setTimeout(r, (current + 1) * TOTP_STEP_MS - Date.now() + 50));
    }
  }

  async login(creds: Credentials): Promise<void> {
    this.#creds = creds;
    const first = await this.#ok("login", "POST", `${ADMIN}/session`, { body: { email: creds.email, password: creds.password } });
    let body = first;
    if (first["mfaRequired"] === true && creds.totpSecret) {
      body = await this.#ok("login (mfa)", "POST", `${ADMIN}/session/mfa`, { body: { totpCode: await this.#nextTotp(creds.totpSecret) } });
    }
    this.#csrf = typeof body["csrfToken"] === "string" ? body["csrfToken"] : "";
  }

  /** 重い操作（多要素認証の設定、トークンとキーの発行）の前の再認証。ログインした本人の認証情報で行う。 */
  async #reauthenticate(): Promise<void> {
    const creds = this.#creds;
    if (!creds) throw new Error("not logged in");
    const method = creds.totpSecret ? { totpCode: await this.#nextTotp(creds.totpSecret) } : { password: creds.password };
    await this.#ok("reauthentication", "POST", `${ADMIN}/session/reauth`, { body: method });
  }

  /** 多要素認証（TOTP）を設定して、秘密の値を返す。 */
  async enrollTotp(): Promise<string> {
    await this.#reauthenticate();
    const { secret } = await this.#ok("totp setup", "POST", `${ADMIN}/me/mfa/totp`);
    if (typeof secret !== "string") throw new Error("totp setup: no secret in the response");
    await this.#ok("totp confirm", "POST", `${ADMIN}/me/mfa/totp/confirm`, { body: { code: await this.#nextTotp(secret) } });
    return secret;
  }

  async createPersonalToken(scopes: string[], workspaceIds: string[]): Promise<string> {
    await this.#reauthenticate();
    const { token } = await this.#ok("personal token", "POST", `${ADMIN}/me/tokens`, { body: { name: "search-eval", scopes, workspaceIds } });
    if (typeof token !== "string") throw new Error("personal token: no token in the response");
    return token;
  }

  async createWorkspace(name: string, template: string): Promise<string> {
    await this.#reauthenticate();
    const { id } = await this.#ok("create workspace", "POST", `${ADMIN}/workspaces`, { body: { name, template } });
    if (typeof id !== "string") throw new Error("create workspace: no id in the response");
    return id;
  }

  async createApiKey(workspaceId: string, scopes: string[]): Promise<string> {
    await this.#reauthenticate();
    const { key } = await this.#ok("api key", "POST", `${ADMIN}/workspaces/${workspaceId}/api-keys`, { body: { scopes } });
    if (typeof key !== "string") throw new Error("api key: no key in the response");
    return key;
  }

  /** 招待のトークン。メールの送信の設定がなければ、招待のリンクが応答に入る（評価用の CMS はメールを設定しない）。 */
  async invite(workspaceId: string, email: string, role: string): Promise<string> {
    const { inviteUrl } = await this.#ok("invite", "POST", `${ADMIN}/workspaces/${workspaceId}/invitations`, { body: { email, role } });
    const token = typeof inviteUrl === "string" ? new URL(inviteUrl).hash.match(/token=([^&]+)/)?.[1] : undefined;
    if (!token) throw new Error("invite: no invite link in the response");
    return token;
  }

  async acceptInvite(token: string, newPassword: string): Promise<void> {
    await this.#ok("accept invite", "POST", `${ADMIN}/invitations/accept`, { body: { token, newPassword } });
  }

  /** 記事の ID と slug の対応。 */
  async entrySlugs(workspaceId: string): Promise<Map<string, string>> {
    const res = await this.#request("GET", `${ADMIN}/workspaces/${workspaceId}/entries`);
    if (res.status !== 200) throw new CmsError("list entries", res);
    const list = JSON.parse(res.text) as { id: string; fields: Record<string, unknown> }[];
    return new Map(list.map((e) => [e.id, String(e.fields["slug"] ?? "")]));
  }

  /** 埋め込みの作成に今月使ったトークン数（記事の埋め込みの分。検索語の分は含まない）。 */
  async embeddingTokensUsed(workspaceId: string): Promise<number> {
    const { usedThisMonth, exhaustedAt } = await this.#ok("embedding budget", "GET", `${ADMIN}/workspaces/${workspaceId}/budgets/embedding`);
    if (typeof usedThisMonth !== "number") throw new Error("embedding budget: no usedThisMonth in the response");
    // 予算が尽きると使用量が増えなくなり、埋め込みが途中なのに終わったように見えるので止める
    if (exhaustedAt) throw new Error(`embedding budget of workspace ${workspaceId} was exhausted at ${String(exhaustedAt)}`);
    return usedThisMonth;
  }

  async registerSearchQuery(workspaceId: string, registerKey: string): Promise<string> {
    const query =
      "query($text: String!, $locale: String!, $first: Int, $mode: YomiSearchMode) { search(text: $text, locale: $locale, first: $first, mode: $mode) { vectorSearch keywordSearch hits { entryId anchor breadcrumb score } } }";
    const { id } = await this.#ok("register query", "POST", `/ws/${workspaceId}/graphql/queries`, { bearer: registerKey, body: { query } });
    if (typeof id !== "string") throw new Error("register query: no id in the response");
    return id;
  }

  /** 検索は POST だけ（検索語を URL に載せない）。 */
  async search(workspaceId: string, readKey: string, queryId: string, input: { text: string; locale: string; first: number; mode: SearchMode }): Promise<SearchResult> {
    const variables = { ...input, mode: input.mode.toUpperCase() };
    const body = await this.#ok("search", "POST", `/ws/${workspaceId}/graphql`, { bearer: readKey, body: { id: queryId, variables } });
    const data = body["data"] as { search?: SearchResult } | undefined;
    if (!data?.search || body["errors"]) throw new Error(`search: unexpected response: ${JSON.stringify(body).slice(0, 300)}`);
    return data.search;
  }
}
