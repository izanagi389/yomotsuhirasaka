// 管理APIの呼び出し（個人トークン）。トークンは環境変数 YOMI_TOKEN で受け取り、ログに出さない。

export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

/** 接続先のURLの検査。HTTPS だけを許し、同じマシンの上の CMS だけ HTTP も許す（トークンを平文で送らないため）。 */
export function checkBaseUrl(raw: string): URL {
  let u: URL;
  try {
    u = new URL(raw);
  } catch {
    throw new Error(`--url: not a URL: ${raw}`);
  }
  const loopback = ["localhost", "127.0.0.1", "[::1]"].includes(u.hostname);
  if (u.protocol !== "https:" && !(u.protocol === "http:" && loopback)) {
    throw new Error("--url: use https:// (http:// is allowed only for localhost)");
  }
  if (u.username || u.password) throw new Error("--url: must not contain credentials");
  return u;
}

export class Client {
  constructor(
    private readonly base: URL,
    private readonly token: string,
  ) {}

  private url(path: string): URL {
    return new URL(this.base.pathname.replace(/\/+$/, "") + path, this.base);
  }

  async request(method: string, path: string, body?: { json: unknown } | { bytes: Buffer; contentType: string }): Promise<unknown> {
    const headers: Record<string, string> = { Authorization: `Bearer ${this.token}` };
    let payload: string | Blob | undefined;
    if (body && "json" in body) {
      headers["Content-Type"] = "application/json";
      payload = JSON.stringify(body.json);
    } else if (body) {
      headers["Content-Type"] = body.contentType;
      payload = new Blob([new Uint8Array(body.bytes)]);
    }
    const res = await fetch(this.url(path), { method, headers, body: payload, redirect: "error" });
    const text = await res.text();
    let parsed: unknown = null;
    try {
      parsed = text ? JSON.parse(text) : null;
    } catch {
      parsed = null;
    }
    if (!res.ok) {
      const e = (parsed ?? {}) as { error?: unknown; message?: unknown };
      throw new ApiError(res.status, typeof e.error === "string" ? e.error : "http_error", typeof e.message === "string" ? e.message : `HTTP ${res.status}`);
    }
    return parsed;
  }
}
