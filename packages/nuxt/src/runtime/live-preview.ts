// ライブプレビューの受け手（ブラウザだけ。管理画面の iframe の中で開かれたときだけ動く）。
// 管理画面（CMSのオリジン）の、親の窓からの「読み直して」の知らせだけを受け付け（INV-PRV-03、packages/preview）、
// CMSから取り直す（中身はメッセージに載らない）。読み込んだら、親の窓に「準備できた」を知らせる。

import { READY, REFRESH, acceptPreviewMessage } from "@yomotsuhirasaka/preview";
import { defineNuxtPlugin, refreshNuxtData, useRuntimeConfig } from "nuxt/app";

export default defineNuxtPlugin(() => {
  if (window.parent === window) return;
  const yomiOrigin = (useRuntimeConfig().public as { yomi?: { yomiOrigin?: string } }).yomi?.yomiOrigin;
  if (!yomiOrigin) return;
  window.addEventListener("message", (event) => {
    const message = acceptPreviewMessage(event, { origins: [yomiOrigin], source: window.parent });
    if (message?.type === REFRESH) void refreshNuxtData();
  });
  // 宛先のオリジンを指定する（親の窓が CMS でなければ、ブラウザが届けない）
  window.parent.postMessage({ type: READY }, yomiOrigin);
});
