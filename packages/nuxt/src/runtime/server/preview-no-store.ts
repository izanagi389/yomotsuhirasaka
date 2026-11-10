// プレビュー中の応答を、共有のキャッシュ（CDN・リバースプロキシ）にもブラウザにも残させない。
// プレビューのクッキーを持つリクエストへの応答には、下書きが含まれうる。ページのHTML（SSR）も含めて、
// 応答の直前に `Cache-Control: private, no-store` にする（ほかの設定で上書きされないように、最後に付ける）。
// URL だけでキャッシュする CDN が、下書きの HTML を他の訪問者に返すことを防ぐ。

import { type H3Event, setResponseHeader } from "h3";
import { defineNitroPlugin } from "nitropack/runtime";

import { previewTokenOf } from "./preview-cookie.ts";

/** プレビューのクッキーがあれば、応答をキャッシュさせない。 */
export function markPreviewResponse(event: H3Event): void {
  if (previewTokenOf(event)) setResponseHeader(event, "Cache-Control", "private, no-store");
}

export default defineNitroPlugin((nitroApp) => {
  nitroApp.hooks.hook("request", markPreviewResponse);
  nitroApp.hooks.hook("beforeResponse", markPreviewResponse);
});
