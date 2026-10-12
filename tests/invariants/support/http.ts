// HTTPの応答を、テストで調べやすい形にまとめる。

export interface HttpResponse {
  status: number;
  headers: Headers;
  /** Set-Cookie ヘッダーをすべて（1件ずつ）。 */
  setCookies: string[];
  /** 応答の本文（そのままの文字列）。 */
  text: string;
}

export async function toHttpResponse(res: Response): Promise<HttpResponse> {
  return {
    status: res.status,
    headers: res.headers,
    setCookies: res.headers.getSetCookie(),
    text: await res.text(),
  };
}

export function isSuccess(res: HttpResponse): boolean {
  return res.status >= 200 && res.status < 300;
}

/** 拒否とみなす状態コード。認証・認可の失敗は、GraphQLの実行より前にHTTPの状態コードで返す。 */
export const REJECTED_STATUSES = [401, 403, 404] as const;

export function isRejected(res: HttpResponse): boolean {
  return (REJECTED_STATUSES as readonly number[]).includes(res.status);
}

export interface ParsedCookie {
  name: string;
  value: string;
  /** 属性名を小文字にしたもの → 値（値のない属性は空文字）。 */
  attributes: Map<string, string>;
}

export function parseSetCookie(header: string): ParsedCookie {
  const [pair = "", ...attrs] = header.split(";").map((s) => s.trim());
  const eq = pair.indexOf("=");
  const attributes = new Map<string, string>();
  for (const a of attrs) {
    const i = a.indexOf("=");
    if (i < 0) attributes.set(a.toLowerCase(), "");
    else attributes.set(a.slice(0, i).toLowerCase(), a.slice(i + 1));
  }
  return { name: pair.slice(0, eq), value: pair.slice(eq + 1), attributes };
}
