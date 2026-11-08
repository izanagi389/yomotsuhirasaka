// サイトのプレビュー用ルート（03-content-and-editing.md「プレビュー（Q72）」、計画の13番）。
//
// - `<route>?token=<プレビュー用トークン>`: CMSでトークンを確かめ、有効なら HttpOnly のクッキーに替えて、
//   記事のページ（URLのパスの部分だけ）へリダイレクトする。トークンは URL に残さない（Referrer も送らない）。
//   以降、`<routePrefix>/<クエリ>` はクッキーのトークンを付けて取り、対象の記事だけが下書きで返る。
// - `<route>/exit`: クッキーを消して、トップへ。
// SSG のサイトでも、このルートとクエリのルートはサイトのサーバーで動かす（Q145）。

import { createError, defineEventHandler, getQuery, sendRedirect, setResponseHeader } from "h3";
import { useRuntimeConfig } from "nitropack/runtime";

import { checkPreviewToken, cookieSeconds, redirectPath } from "./preview-check.ts";
import { clearPreviewCookie, setPreviewCookie } from "./preview-cookie.ts";

export default defineEventHandler(async (event) => {
  setResponseHeader(event, "Cache-Control", "no-store");
  setResponseHeader(event, "Referrer-Policy", "no-referrer");
  const config = useRuntimeConfig(event) as unknown as { yomi: { url: string; workspaceId: string; readKey: string } };
  if (event.path.split("?")[0]?.endsWith("/exit")) {
    clearPreviewCookie(event);
    return sendRedirect(event, "/", 302);
  }
  if (!config.yomi.readKey) throw createError({ statusCode: 500, statusMessage: "The CMS read key is not configured" });
  const token = getQuery(event)["token"];
  const info = typeof token === "string" ? await checkPreviewToken(config.yomi, token) : null;
  if (!info || !token || typeof token !== "string") {
    clearPreviewCookie(event);
    throw createError({ statusCode: 401, statusMessage: "This preview link is not valid. Open the preview again from the CMS." });
  }
  setPreviewCookie(event, token, cookieSeconds(info.expiresAt));
  return sendRedirect(event, redirectPath(info.url), 302);
});
