// CMSの公式Nuxtモジュール（M1の最小限: 配信APIからの取得と、本文の描画）。
//
// nuxt.config.ts:
//   modules: ["@new-cms/nuxt"],
//   cms: { url: "https://cms.example.com", workspaceId: "<ワークスペースID>" },
// 環境変数:
//   CMS_REGISTER_KEY   ビルドのときだけ。クエリの登録用（register の範囲のキー）
//   NUXT_CMS_READ_KEY  サイトのサーバーの実行時。読むためのキー（read の範囲）。非公開の設定（runtimeConfig.cms.readKey）に入る
//
// - キーは、ブラウザに渡る設定（runtimeConfig.public）やバンドルに入れない。公開の設定にキーがあれば、ビルドを止める
// - 画面からは useCmsQuery で取る。取るのはサイトのサーバー（`<routePrefix>/<名前>`）で、そこから配信APIを呼ぶ
// - 本文は <CmsBody :doc="..."> で描く
// - リダイレクト（siteUrl を設定したとき）: CMSのリダイレクト一覧を、ビルドのときに routeRules の 301 にし、サーバーのミドルウェアで
//   301 と 410 を返す。一覧はキャッシュし、CMSのWebhook（<webhookRoute>、NUXT_CMS_WEBHOOK_SECRET）を受けたら消す
// - 訂正（corrections を設定したとき）: 訂正一覧は useCmsCorrections と <CmsCorrections> で、ページはサイトが作る。
//   フィード（<path>.xml と <path>.json）はサーバーのルートが返し、nuxt generate ではビルドのときに書き出す
// - 削除済みアンカーの案内: <CmsDeletedAnchorNotice :anchors="post.deletedAnchors">

import { resolve } from "node:path";

import { addComponent, addImports, addServerHandler, createResolver, defineNuxtModule, useLogger } from "nuxt/kit";

import { RegisterError, checkUrl, readQueries, registerQueries } from "./register.ts";
import { type CorrectionFeedConfig, checkFeedConfig } from "./runtime/server/feed.ts";
import { fetchQuery } from "./runtime/server/fetch-query.ts";
import { CORRECTIONS_QUERY, CORRECTIONS_QUERY_TEXT, REDIRECTS_QUERY, REDIRECTS_QUERY_TEXT, RESERVED_QUERIES } from "./runtime/server/names.ts";
import { type CmsRedirect, toPathRules, toRouteRules } from "./runtime/server/redirect-rules.ts";

export interface ModuleOptions {
  /** CMSのURL。 */
  url: string;
  /** ワークスペースのID。 */
  workspaceId: string;
  /** クエリのファイルを置くフォルダ（サイトのルートからの相対パス）。 */
  queriesDir: string;
  /** サイトのサーバーの、クエリを取るルートの接頭辞。 */
  routePrefix: string;
  /** サイトのURL（例: https://example.com）。設定すると、CMSのリダイレクト一覧のうち、このオリジンのURLを転送する。 */
  siteUrl: string;
  /** CMSからのWebhookの受け口のパス。秘密は NUXT_CMS_WEBHOOK_SECRET で渡す（設定しなければ受け口は 404）。 */
  webhookRoute: string;
  /** 訂正の一覧とフィード。設定すると、訂正の一覧のクエリを登録する。 */
  corrections: false | { feeds?: Record<string, CorrectionFeedOptions> };
}

/** 訂正フィードの設定（キーはロケール）。 */
export interface CorrectionFeedOptions {
  /** フィードのパス（拡張子なし。例: /corrections → /corrections.xml と /corrections.json）。 */
  path: string;
  /** フィードの名前。 */
  title: string;
  /** サイトのURL。 */
  link: string;
  /** フィードの説明（なければ title）。 */
  description?: string;
  /** 載せる件数（1〜100。既定は 50）。 */
  limit?: number;
}

export type { CmsCorrectionItem } from "./runtime/server/feed.ts";
export type { CmsDeletedAnchor } from "./runtime/deleted-anchor.ts";

/** CMSのAPIキー（`cms_key_`）か個人トークン（`cms_pat_`）らしい文字列を含むか。 */
export function containsSecret(value: unknown): boolean {
  if (typeof value === "string") return /^cms_(key|pat)_/.test(value);
  if (Array.isArray(value)) return value.some(containsSecret);
  if (value && typeof value === "object") return Object.values(value).some(containsSecret);
  return false;
}

export default defineNuxtModule<ModuleOptions>({
  meta: { name: "@new-cms/nuxt", configKey: "cms" },
  defaults: { url: "", workspaceId: "", queriesDir: "cms/queries", routePrefix: "/_cms/q", siteUrl: "", webhookRoute: "/_cms/webhook", corrections: false },
  async setup(options, nuxt) {
    const logger = useLogger("cms");
    const resolver = createResolver(import.meta.url);

    // キーが公開の設定に入っていないか（ブラウザに渡ってしまう）
    if (containsSecret(nuxt.options.runtimeConfig.public)) {
      throw new RegisterError("a CMS key or token is in runtimeConfig.public, which is sent to browsers. Use NUXT_CMS_READ_KEY (private) instead.");
    }

    const url = options.url ? checkUrl(options.url) : "";
    const siteUrl = options.siteUrl ? new URL(options.siteUrl).origin : "";
    const feeds: Record<string, CorrectionFeedConfig> = {};
    for (const [locale, raw] of Object.entries((options.corrections && options.corrections.feeds) || {})) {
      const feed = checkFeedConfig(locale, raw ?? {});
      if (Object.hasOwn(feeds, feed.path)) throw new RegisterError(`cms.corrections.feeds.${locale}.path: used by another feed`);
      feeds[feed.path] = feed;
    }
    nuxt.options.runtimeConfig.cms = { url, workspaceId: options.workspaceId, readKey: "", siteUrl, webhookSecret: "", correctionFeeds: feeds };
    nuxt.options.runtimeConfig.public.cms = { routePrefix: options.routePrefix };

    // クエリの登録（nuxt prepare では行わない）
    const queries = readQueries(resolve(nuxt.options.rootDir, options.queriesDir));
    for (const name of RESERVED_QUERIES) {
      if (Object.hasOwn(queries, name)) throw new RegisterError(`${options.queriesDir}/${name}.graphql: this name is used by the module`);
    }
    // リダイレクト一覧と訂正の一覧のクエリは、モジュールが自分で登録する
    if (siteUrl) queries[REDIRECTS_QUERY] = REDIRECTS_QUERY_TEXT;
    if (options.corrections) queries[CORRECTIONS_QUERY] = CORRECTIONS_QUERY_TEXT;
    let ids: Record<string, string> = {};
    if (Object.keys(queries).length > 0 && !nuxt.options._prepare) {
      const registerKey = process.env["CMS_REGISTER_KEY"];
      if (!url || !options.workspaceId) throw new RegisterError("set cms.url and cms.workspaceId in nuxt.config");
      if (!registerKey) throw new RegisterError("set CMS_REGISTER_KEY (an API key with the register scope) to register the queries in " + options.queriesDir);
      ids = await registerQueries({ url, workspaceId: options.workspaceId, registerKey }, queries);
      logger.info(`registered ${Object.keys(ids).length} CMS quer${Object.keys(ids).length === 1 ? "y" : "ies"}`);
    }
    nuxt.options.nitro.virtual = { ...nuxt.options.nitro.virtual, "#cms/queries": () => `export default ${JSON.stringify(ids)};` };

    // リダイレクト: ビルドのときに読むキーがあれば routeRules の 301 を作る（サイトが書いた routeRules は上書きしない）。
    // 410 と、ビルドの後の変更は、サーバーのミドルウェアが返す
    if (siteUrl && ids[REDIRECTS_QUERY]) {
      const readKey = process.env["NUXT_CMS_READ_KEY"];
      if (readKey) {
        const data = (await fetchQuery({ url, workspaceId: options.workspaceId, readKey }, ids[REDIRECTS_QUERY], undefined)) as { redirects?: CmsRedirect[] } | null;
        const rules = toRouteRules(toPathRules(data?.redirects ?? [], siteUrl));
        nuxt.options.routeRules = { ...rules, ...nuxt.options.routeRules };
        logger.info(`added ${Object.keys(rules).length} redirect(s) from the CMS to routeRules`);
      } else {
        logger.info("NUXT_CMS_READ_KEY is not set at build time; CMS redirects are served by the server middleware only");
      }
      addServerHandler({ middleware: true, handler: resolver.resolve("./runtime/server/redirects.js") });
    }
    // 訂正フィード: サーバーのルートで返す。nuxt generate ではビルドのときに書き出す（Q145）
    const feedRoutes = Object.keys(feeds).flatMap((path) => [`${path}.xml`, `${path}.json`]);
    for (const route of feedRoutes) addServerHandler({ route, method: "get", handler: resolver.resolve("./runtime/server/corrections-feed.js") });
    if (feedRoutes.length > 0 && nuxt.options.nitro.static) {
      nuxt.options.nitro.prerender = { ...nuxt.options.nitro.prerender, routes: [...(nuxt.options.nitro.prerender?.routes ?? []), ...feedRoutes] };
    }
    addServerHandler({ route: options.webhookRoute, method: "post", handler: resolver.resolve("./runtime/server/webhook.js") });

    addServerHandler({ route: `${options.routePrefix}/:name`, method: "get", handler: resolver.resolve("./runtime/server/query.js") });
    addImports([
      { name: "useCmsQuery", from: resolver.resolve("./runtime/composables.js") },
      { name: "useCmsCorrections", from: resolver.resolve("./runtime/composables.js") },
    ]);
    addComponent({ name: "CmsBody", filePath: resolver.resolve("./runtime/body.js") });
    addComponent({ name: "CmsCorrections", filePath: resolver.resolve("./runtime/corrections.js") });
    addComponent({ name: "CmsDeletedAnchorNotice", filePath: resolver.resolve("./runtime/deleted-anchor.js") });
  },
});
