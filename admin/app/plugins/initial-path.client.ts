// 管理画面を最初に読み込んだときのパス。記事の画面の CSP の frame-src は、記事の画面を直接開いたときにだけ付くので
// （server/src/http/admin_ui.rs）、ライブプレビューを開くときに、読み直しが要るかを見分けるのに使う。
export default defineNuxtPlugin(() => {
  useState("initial-path", () => window.location.pathname);
});
