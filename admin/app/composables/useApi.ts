// 管理APIの呼び出し。Cookie のセッション（HttpOnly）と、状態を変える操作に付ける CSRF トークン。
//
// - CSRF トークンはログインの応答でだけ受け取る。タブを閉じるまで残るよう sessionStorage に置く（持ち主の判断）。
//   新しいタブでは、状態を変える操作の前にログインし直してもらう。
// - 再認証が要る操作（`reauth_required`）は、再認証のダイアログを出し、通ったらもう一度送る。

const CSRF_KEY = "cms:csrf";
const API = "/api/admin";

export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

export function csrfToken(): string | null {
  try {
    return sessionStorage.getItem(CSRF_KEY);
  } catch {
    return null;
  }
}

export function setCsrfToken(token: string | null): void {
  try {
    if (token) sessionStorage.setItem(CSRF_KEY, token);
    else sessionStorage.removeItem(CSRF_KEY);
  } catch {
    // sessionStorage が使えない環境では、ページを開いている間だけになる
  }
}

type Body = { json: unknown } | { form: Blob; contentType: string } | undefined;

interface Options {
  /** 再認証を求められても、ダイアログを出さない。 */
  noReauth?: boolean;
}

async function send<T>(method: string, path: string, body: Body, options: Options): Promise<T> {
  const headers: Record<string, string> = { Accept: "application/json" };
  const safe = method === "GET" || method === "HEAD";
  if (!safe) {
    const token = csrfToken();
    if (token) headers["X-CSRF-Token"] = token;
  }
  let payload: BodyInit | undefined;
  if (body && "json" in body) {
    headers["Content-Type"] = "application/json";
    payload = JSON.stringify(body.json);
  } else if (body) {
    headers["Content-Type"] = body.contentType;
    payload = body.form;
  }
  const res = await fetch(API + path, { method, headers, body: payload, credentials: "same-origin", redirect: "error" });
  if (res.status === 204) return undefined as T;
  const text = await res.text();
  let data: unknown = null;
  try {
    data = text ? JSON.parse(text) : null;
  } catch {
    data = null;
  }
  if (!res.ok) {
    const e = (data ?? {}) as { error?: unknown; message?: unknown };
    const err = new ApiError(res.status, typeof e.error === "string" ? e.error : "http_error", typeof e.message === "string" ? e.message : `HTTP ${res.status}`);
    if (err.code === "reauth_required" && !options.noReauth) {
      // 再認証が通ったら、同じリクエストをもう一度送る（2回目は再認証のダイアログを出さない）
      if (await useReauth().request()) return send<T>(method, path, body, { ...options, noReauth: true });
    }
    if (err.status === 401 && err.code !== "invalid_credentials") useAuth().forget();
    throw err;
  }
  return data as T;
}

export function useApi() {
  return {
    get: <T>(path: string, options: Options = {}) => send<T>("GET", path, undefined, options),
    post: <T>(path: string, json?: unknown, options: Options = {}) => send<T>("POST", path, json === undefined ? undefined : { json }, options),
    put: <T>(path: string, json: unknown, options: Options = {}) => send<T>("PUT", path, { json }, options),
    patch: <T>(path: string, json: unknown, options: Options = {}) => send<T>("PATCH", path, { json }, options),
    del: <T>(path: string, options: Options = {}) => send<T>("DELETE", path, undefined, options),
    upload: <T>(path: string, file: Blob, contentType: string, options: Options = {}) => send<T>("POST", path, { form: file, contentType }, options),
  };
}
