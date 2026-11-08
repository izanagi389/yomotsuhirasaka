// プレビュー用のクッキー（サイトのサーバーだけが読む。HttpOnly）。
// ライブプレビューでは管理画面の iframe の中（別のサイト）で使うので、SameSite=None と Partitioned を付ける
// （パーティション化されたクッキー。対応しないブラウザでは iframe の中では残らないので、別タブで開いてもらう）。

import { type H3Event, deleteCookie, getCookie, setCookie } from "h3";

export const PREVIEW_COOKIE = "yomi_preview";
/** トークンの接頭辞（server/src/content/previews.rs）。 */
export const PREVIEW_TOKEN_PREFIX = "yomi_prv_";

const ATTRS = { httpOnly: true, secure: true, sameSite: "none", partitioned: true, path: "/" } as const;

/** クッキーのトークン（形が違えば無いものとする）。 */
export function previewTokenOf(event: H3Event): string | undefined {
  const v = getCookie(event, PREVIEW_COOKIE);
  return v && v.startsWith(PREVIEW_TOKEN_PREFIX) && v.length < 200 ? v : undefined;
}

export function setPreviewCookie(event: H3Event, token: string, maxAgeSeconds: number): void {
  setCookie(event, PREVIEW_COOKIE, token, { ...ATTRS, maxAge: maxAgeSeconds });
}

export function clearPreviewCookie(event: H3Event): void {
  deleteCookie(event, PREVIEW_COOKIE, ATTRS);
}
