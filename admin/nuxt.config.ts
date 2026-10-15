// 管理画面（Nuxt の SPA）。`nuxt generate` の出力（.output/public）を、Rust のサーバーが /admin/ で配信する。
// 本番で Node.js は動かさない（01-architecture.md「管理画面」）。
import tailwindcss from "@tailwindcss/vite";

export default defineNuxtConfig({
  compatibilityDate: "2026-10-01",
  ssr: false,
  devtools: { enabled: false },
  telemetry: false,
  app: {
    baseURL: "/admin/",
    head: {
      title: "CMS",
      meta: [{ name: "robots", content: "noindex, nofollow" }],
    },
  },
  css: ["~/assets/css/main.css", "~/assets/css/editor.css"],
  // shadcn-vue の部品（components/ui）は、使う画面で明示して読み込む（自動の読み込みから外す）
  components: [{ path: "~/components", ignore: ["ui/**"] }],
  vite: {
    plugins: [tailwindcss()],
  },
  modules: ["@nuxtjs/i18n"],
  i18n: {
    // 画面の文言はすべて翻訳ファイルに置く（03-content-and-editing.md「管理画面の言語」、Q79）
    locales: [
      { code: "ja", language: "ja", name: "日本語", file: "ja.json" },
      { code: "en", language: "en", name: "English", file: "en.json" },
    ],
    defaultLocale: "ja",
    strategy: "no_prefix",
    // ブラウザの言語で決め、ログインした後は本人の設定に合わせる。Cookie には残さない
    detectBrowserLanguage: { useCookie: false, fallbackLocale: "en" },
  },
  // 手元の開発では、管理APIを Rust のサーバーに中継する（`nuxt dev`）
  nitro: {
    devProxy: {
      "/api": { target: "http://127.0.0.1:8080/api", changeOrigin: false },
    },
  },
  typescript: {
    strict: true,
  },
});
