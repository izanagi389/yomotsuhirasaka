// プレビュー用トークンをCMSで確かめる（サイトのサーバーの中だけで動く）。ここは Nitro に依存しない（テストで直接呼ぶ）。

import { PREVIEW_HEADER, type QueryTarget } from "./fetch-query.ts";
import { PREVIEW_TOKEN_PREFIX } from "./preview-cookie.ts";

export interface PreviewInfo {
  entryId: string;
  /** 下書きから作った公開URL（作れなければ null）。 */
  url: string | null;
  expiresAt: string;
}

/** トークンが有効なら、対象の記事の情報。無効・CMSに届かないなら null。 */
export async function checkPreviewToken(target: Omit<QueryTarget, "previewToken">, token: string, fetchImpl: typeof fetch = fetch): Promise<PreviewInfo | null> {
  if (!token.startsWith(PREVIEW_TOKEN_PREFIX) || token.length > 200) return null;
  const u = new URL(`${target.url.replace(/\/+$/, "")}/ws/${encodeURIComponent(target.workspaceId)}/preview`);
  let res: Response;
  try {
    res = await fetchImpl(u, {
      headers: { Authorization: `Bearer ${target.readKey}`, Accept: "application/json", [PREVIEW_HEADER]: token },
      redirect: "error",
      signal: AbortSignal.timeout(15_000),
    });
  } catch {
    return null;
  }
  if (!res.ok) return null;
  const body = (await res.json().catch(() => null)) as Partial<PreviewInfo> | null;
  if (typeof body?.entryId !== "string" || typeof body.expiresAt !== "string") return null;
  return { entryId: body.entryId, url: typeof body.url === "string" ? body.url : null, expiresAt: body.expiresAt };
}

/**
 * リダイレクト先（記事のURLのパスとクエリの部分だけ。別のサイトへは飛ばさない）。
 * URLがない・読めないときはトップ（`/`）。
 */
export function redirectPath(url: string | null): string {
  if (!url) return "/";
  let u: URL;
  try {
    u = new URL(url);
  } catch {
    return "/";
  }
  const path = `${u.pathname}${u.search}`;
  // `//evil.example` のような、別のホストへ飛ぶパスにしない
  return path.startsWith("/") && !path.startsWith("//") && !path.startsWith("/\\") ? path : "/";
}

/** クッキーを持たせる秒数（有効期限まで。最長10分）。 */
export function cookieSeconds(expiresAt: string, now: number = Date.now()): number {
  const left = Math.floor((Date.parse(expiresAt) - now) / 1000);
  return Number.isFinite(left) ? Math.min(Math.max(left, 0), 600) : 0;
}
