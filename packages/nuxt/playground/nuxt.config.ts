// モジュールの確かめ用のサイト（packages/nuxt/README.md）。
export default defineNuxtConfig({
  compatibilityDate: "2026-10-01",
  modules: ["@yomotsuhirasaka/nuxt"],
  yomi: {
    url: process.env["YOMI_URL"] ?? "http://localhost:8080",
    workspaceId: process.env["YOMI_WORKSPACE_ID"] ?? "",
    siteUrl: process.env["YOMI_SITE_URL"] ?? "",
    ai: { locales: { ja: { title: "確かめ用のサイト", description: "モジュールの確かめ用" } }, robotsTxt: {} },
    search: {},
    corrections: { feeds: { ja: { path: "/corrections", title: "訂正の一覧", link: process.env["YOMI_SITE_URL"] || "http://localhost:3000" } } },
  },
  nitro: { prerender: { crawlLinks: true, routes: ["/", "/corrections"] } },
});
