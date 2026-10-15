// 登録済みのクエリを、配信APIから取る（サイトのサーバーの中だけで動く。APIキーはブラウザに渡さない。原則1）。
// ここは Nitro に依存しない（テストで直接呼ぶ）。

export interface QueryTarget {
  /** CMSのURL（https。手元だけ http も可）。 */
  url: string;
  workspaceId: string;
  /** 読むためのAPIキー（read の範囲）。 */
  readKey: string;
}

export class QueryError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

/** 変数（JSON）の大きさの上限。登録したクエリの変数だけなので、大きなものは要らない。 */
export const MAX_VARIABLES_CHARS = 4096;

/** 変数の文字列を確かめる（JSONのオブジェクトだけ）。 */
export function parseVariables(raw: unknown): Record<string, unknown> | undefined {
  if (raw === undefined || raw === null || raw === "") return undefined;
  if (typeof raw !== "string" || raw.length > MAX_VARIABLES_CHARS) throw new QueryError(400, "variables are too large or not a string");
  let v: unknown;
  try {
    v = JSON.parse(raw);
  } catch {
    throw new QueryError(400, "variables are not JSON");
  }
  if (typeof v !== "object" || v === null || Array.isArray(v)) throw new QueryError(400, "variables must be a JSON object");
  return v as Record<string, unknown>;
}

/** 登録済みのクエリを実行し、GraphQL の `data` を返す。 */
export async function fetchQuery(
  target: QueryTarget,
  id: string,
  variables: Record<string, unknown> | undefined,
  fetchImpl: typeof fetch = fetch,
): Promise<unknown> {
  const u = new URL(`${target.url.replace(/\/+$/, "")}/ws/${encodeURIComponent(target.workspaceId)}/graphql`);
  u.searchParams.set("id", id);
  if (variables) u.searchParams.set("variables", JSON.stringify(variables));
  let res: Response;
  try {
    res = await fetchImpl(u, {
      headers: { Authorization: `Bearer ${target.readKey}`, Accept: "application/json" },
      redirect: "error",
      signal: AbortSignal.timeout(15_000),
    });
  } catch {
    throw new QueryError(502, "cannot reach the CMS");
  }
  type Reply = { data?: unknown; errors?: { message?: unknown }[]; error?: unknown; message?: unknown } | null;
  let body: Reply = null;
  try {
    body = (await res.json()) as Reply;
  } catch {
    body = null;
  }
  if (!res.ok) {
    // CMSの認証・認可の失敗は、サイトの設定の誤り。訪問者には 502 として返す（キーの状態を外に知らせない）
    const code = typeof body?.error === "string" ? body.error : `HTTP ${res.status}`;
    throw new QueryError(res.status === 429 ? 503 : 502, `the CMS refused the query (${code})`);
  }
  if (body?.errors && body.errors.length > 0) {
    throw new QueryError(502, `the query failed: ${body.errors.map((e) => String(e.message ?? "")).join("; ")}`);
  }
  return body?.data ?? null;
}
