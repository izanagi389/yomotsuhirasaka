// モジュールの確かめ用のサイト（packages/nuxt/README.md）。
export default defineNuxtConfig({
  compatibilityDate: "2026-10-01",
  modules: ["@new-cms/nuxt"],
  cms: {
    url: process.env["CMS_URL"] ?? "http://localhost:8080",
    workspaceId: process.env["CMS_WORKSPACE_ID"] ?? "",
  },
  nitro: { prerender: { crawlLinks: true, routes: ["/"] } },
});
