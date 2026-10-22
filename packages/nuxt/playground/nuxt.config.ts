// モジュールの確かめ用のサイト（packages/nuxt/README.md）。
export default defineNuxtConfig({
  compatibilityDate: "2026-10-01",
  modules: ["@new-cms/nuxt"],
  cms: {
    url: process.env["CMS_URL"] ?? "http://localhost:8080",
    workspaceId: process.env["CMS_WORKSPACE_ID"] ?? "",
    siteUrl: process.env["CMS_SITE_URL"] ?? "",
    corrections: { feeds: { ja: { path: "/corrections", title: "訂正の一覧", link: process.env["CMS_SITE_URL"] || "http://localhost:3000" } } },
  },
  nitro: { prerender: { crawlLinks: true, routes: ["/", "/corrections"] } },
});
