// ビルドのときに、クエリのファイル（`<queriesDir>/<名前>.graphql`）を配信APIに登録する（02-schema-and-delivery-api.md「配信API」、
// 13-security.md「GraphQL」）。登録には、読むキーとは別の register の範囲のキーを使う。キーはビルドの環境変数で受け取り、
// サイトの設定やバンドルには書かない。ビルドのたびの登録は「使用」として数えられる（Q151）。

import { readFileSync, readdirSync } from "node:fs";
import { basename, join } from "node:path";

export class RegisterError extends Error {}

/** クエリの名前（ファイル名）。サイトのルートのパスに使うので、使える文字を絞る。 */
const NAME = /^[A-Za-z][A-Za-z0-9_-]{0,63}$/;

/** フォルダの中の `.graphql` を読む（名前 → クエリ）。フォルダがなければ空。 */
export function readQueries(dir: string): Record<string, string> {
  let files: string[];
  try {
    files = readdirSync(dir).filter((f) => f.endsWith(".graphql")).sort();
  } catch {
    return {};
  }
  const out: Record<string, string> = {};
  for (const f of files) {
    const name = basename(f, ".graphql");
    if (!NAME.test(name)) throw new RegisterError(`${f}: query file names must be letters, digits, _ and - (starting with a letter)`);
    out[name] = readFileSync(join(dir, f), "utf8");
  }
  return out;
}

export interface RegisterTarget {
  url: string;
  workspaceId: string;
  registerKey: string;
}

/** 接続先のURLの検査（キーを平文で送らないため、https だけ。手元の CMS だけ http も可）。 */
export function checkUrl(raw: string): string {
  let u: URL;
  try {
    u = new URL(raw);
  } catch {
    throw new RegisterError(`cms.url is not a URL: ${raw}`);
  }
  const loopback = ["localhost", "127.0.0.1", "[::1]"].includes(u.hostname);
  if (u.protocol !== "https:" && !(u.protocol === "http:" && loopback)) throw new RegisterError("cms.url must be https:// (http:// only for localhost)");
  if (u.username || u.password) throw new RegisterError("cms.url must not contain credentials");
  return u.toString().replace(/\/+$/, "");
}

/** クエリを登録し、名前 → 登録したID を返す。 */
export async function registerQueries(
  target: RegisterTarget,
  queries: Record<string, string>,
  fetchImpl: typeof fetch = fetch,
): Promise<Record<string, string>> {
  const out: Record<string, string> = {};
  for (const [name, query] of Object.entries(queries)) {
    const res = await fetchImpl(`${target.url}/ws/${encodeURIComponent(target.workspaceId)}/graphql/queries`, {
      method: "POST",
      headers: { Authorization: `Bearer ${target.registerKey}`, "Content-Type": "application/json", Accept: "application/json" },
      body: JSON.stringify({ query }),
      redirect: "error",
      signal: AbortSignal.timeout(30_000),
    });
    type Reply = { id?: unknown; error?: unknown; message?: unknown } | null;
    let body: Reply = null;
    try {
      body = (await res.json()) as Reply;
    } catch {
      body = null;
    }
    if (!res.ok || typeof body?.id !== "string") {
      const why = typeof body?.message === "string" ? body.message : `HTTP ${res.status}`;
      throw new RegisterError(`cannot register query "${name}": ${why}`);
    }
    out[name] = body.id;
  }
  return out;
}
