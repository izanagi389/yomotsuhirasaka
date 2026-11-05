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
// - AI向けの素材（ai を設定したとき。siteUrl が要る）: `<記事のURL>.md`、/llms.txt、（設定すれば）/robots.txt。
//   サーバーのルートが返し、nuxt generate ではビルドのときに書き出す。学習拒否のメタタグは useCmsAiMeta(post)
// - プレビュー（preview を設定したとき）: `<preview.route>?token=…` がCMSのプレビュー用トークンを確かめてクッキーに替え、
//   記事のページへリダイレクトする。クエリのルートは、クッキーがあればトークンを付けて取る（対象の記事だけが下書きになる）。
//   管理画面の iframe の中では、CMSからの「読み直して」の知らせで取り直す（ライブプレビュー）。SSG でもサイトのサーバーが要る
// - サイト内検索（search を設定したとき）: サイトのサーバーのルート（既定は /_cms/search）が検索のクエリを呼ぶ。
//   画面からは useCmsSearch か <CmsSearchBox>。抜粋の一致した箇所は cmsHighlight で区切る。訪問者のIPアドレスごとに
//   1分あたりの回数を制限する（rateLimit）。サイトのサーバーが要るので、nuxt generate では使えない
// - チャットボットの回答（answer を設定したとき。M4）: サイトのサーバーのルート（既定は POST /_cms/answer）が、回答のキー
//   （NUXT_CMS_ANSWER_KEY、answer の範囲）で CMS の判定先行のストリーミングを呼び、SSE のまま中継する。回答キャッシュから
//   返したか（cached）とCMSのエラーのメッセージは渡さない。訪問者の識別子は、IPアドレス（ipHeader）を回答のキーで HMAC にかけたもの。
//   画面からは useCmsAnswer、本文は <CmsAnswerText>（Markdown の一部だけを、v-html を使わずに描く）。nuxt generate では使えない

import { existsSync } from "node:fs";
import { resolve } from "node:path";

import { addComponent, addImports, addPlugin, addServerHandler, createResolver, defineNuxtModule, useLogger } from "nuxt/kit";

import { RegisterError, checkUrl, readQueries, registerQueries } from "./register.ts";
import { type AiConfig, type AiOptions, checkAiConfig, markdownPath } from "./runtime/server/ai.ts";
import { loadCatalog } from "./runtime/server/ai-load.ts";
import { type CorrectionFeedConfig, checkFeedConfig } from "./runtime/server/feed.ts";
import { fetchQuery } from "./runtime/server/fetch-query.ts";
import {
  AI_CATALOG_QUERY,
  AI_CATALOG_QUERY_TEXT,
  AI_MARKDOWN_QUERY,
  AI_MARKDOWN_QUERY_TEXT,
  AI_OPT_OUTS_QUERY,
  AI_OPT_OUTS_QUERY_TEXT,
  ANSWER_QUERY,
  ANSWER_QUERY_TEXT,
  CORRECTIONS_QUERY,
  CORRECTIONS_QUERY_TEXT,
  REDIRECTS_QUERY,
  REDIRECTS_QUERY_TEXT,
  RESERVED_QUERIES,
  SEARCH_QUERY,
  SEARCH_QUERY_TEXT,
} from "./runtime/server/names.ts";
import { DEFAULT_PER_MINUTE } from "./runtime/server/search-core.ts";
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
  /**
   * AI向けの素材（`<記事のURL>.md`・/llms.txt・/robots.txt）。siteUrl が要る。
   * locales: ロケール → llms.txt に書く名前と説明。robotsTxt: 設定すると /robots.txt 全体をモジュールが返す。
   */
  ai: false | AiOptions;
  /**
   * プレビュー（下書きの表示とライブプレビュー）。route はトークンを受け取るサイトのパス（既定は /_cms/preview。
   * CMSの管理画面には `<サイトのURL><route>` を設定する）。CMSの管理画面のオリジン（cms.url）からのメッセージだけを受け付ける。
   */
  preview: false | { route?: string };
  /**
   * サイト内検索。route はサイトのサーバーのルート（既定は /_cms/search）。
   * rateLimit.perMinute: 訪問者のIPアドレスごとの1分あたりの回数（既定は 30）。
   * rateLimit.ipHeader: 訪問者のIPアドレスを読むヘッダー（例: cf-connecting-ip、x-forwarded-for）。サイトの手前の
   * CDN・リバースプロキシが付けるヘッダーだけを書く（訪問者が偽れるため）。既定は空で、接続元のアドレスを使う。
   */
  search: false | { route?: string; rateLimit?: { perMinute?: number; ipHeader?: string } };
  /**
   * チャットボットの回答の中継。route はサイトのサーバーのルート（既定は /_cms/answer。POST）。
   * ipHeader: 訪問者のIPアドレスを読むヘッダー（検索と同じ。サイトの手前の CDN・リバースプロキシが付けるものだけ）。
   * 訪問者ごとの回数は CMS が数える（訪問者の識別子はIPアドレスから作る）。
   */
  answer: false | { route?: string; ipHeader?: string };
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
export type { CmsAiCatalogItem } from "./runtime/server/ai.ts";
export { DEFAULT_AI_TRAINING_CRAWLERS } from "./runtime/server/ai.ts";
export type { CmsDeletedAnchor } from "./runtime/deleted-anchor.ts";
export type { CmsHighlightSegment, CmsSearchAvailability, CmsSearchHit, CmsSearchMode, CmsSearchResult } from "./runtime/search.ts";
export type { CmsAnswer, CmsAnswerErrorCode, CmsAnswerEvent, CmsAnswerSource, CmsAnswerStatus } from "./runtime/answer.ts";

/** CMSのAPIキー（`cms_key_`）・個人トークン（`cms_pat_`）・プレビュー用トークン（`cms_prv_`）らしい文字列を含むか。 */
export function containsSecret(value: unknown): boolean {
  if (typeof value === "string") return /^cms_(key|pat|prv)_/.test(value);
  if (Array.isArray(value)) return value.some(containsSecret);
  if (value && typeof value === "object") return Object.values(value).some(containsSecret);
  return false;
}

export default defineNuxtModule<ModuleOptions>({
  meta: { name: "@new-cms/nuxt", configKey: "cms" },
  defaults: { url: "", workspaceId: "", queriesDir: "cms/queries", routePrefix: "/_cms/q", siteUrl: "", webhookRoute: "/_cms/webhook", corrections: false, ai: false, preview: false, search: false, answer: false },
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
    const ai: AiConfig | null = options.ai ? checkAiConfig(options.ai, siteUrl) : null;
    if (ai?.robotsTxt && existsSync(resolve(nuxt.options.rootDir, nuxt.options.dir.public, "robots.txt"))) {
      throw new RegisterError("cms.ai.robotsTxt: remove public/robots.txt (the module serves /robots.txt; put your own lines in cms.ai.robotsTxt.extra)");
    }
    const searchRoute = options.search ? (options.search.route ?? "/_cms/search") : "";
    if (searchRoute && (!searchRoute.startsWith("/") || searchRoute.startsWith("//"))) {
      throw new RegisterError("cms.search.route: use a path that starts with /");
    }
    if (searchRoute && nuxt.options.nitro.static) {
      throw new RegisterError("cms.search needs the site's server (it calls the CMS for each search); it cannot be used with nuxt generate");
    }
    const rateLimit = options.search ? (options.search.rateLimit ?? {}) : {};
    const perMinute = rateLimit.perMinute ?? DEFAULT_PER_MINUTE;
    if (!Number.isInteger(perMinute) || perMinute < 1) throw new RegisterError("cms.search.rateLimit.perMinute: use a positive integer");
    const ipHeader = (rateLimit.ipHeader ?? "").trim().toLowerCase();
    if (searchRoute && !ipHeader && !nuxt.options.dev) {
      logger.warn(
        "cms.search.rateLimit.ipHeader is not set: visitors are counted by the connecting address. Behind a CDN or reverse proxy, every visitor shares one limit; set the header your proxy adds.",
      );
    }
    const answerRoute = options.answer ? (options.answer.route ?? "/_cms/answer") : "";
    if (answerRoute && (!url || !answerRoute.startsWith("/") || answerRoute.startsWith("//"))) {
      throw new RegisterError("cms.answer: set cms.url, and make cms.answer.route a path that starts with /");
    }
    if (answerRoute && nuxt.options.nitro.static) {
      throw new RegisterError("cms.answer needs the site's server (it relays each answer from the CMS); it cannot be used with nuxt generate");
    }
    const answerIpHeader = (options.answer ? (options.answer.ipHeader ?? "") : "").trim().toLowerCase();
    if (answerRoute && !answerIpHeader && !nuxt.options.dev) {
      logger.warn(
        "cms.answer.ipHeader is not set: visitors are identified by the connecting address. Behind a CDN or reverse proxy, every visitor shares one limit at the CMS; set the header your proxy adds.",
      );
    }
    nuxt.options.runtimeConfig.cms = {
      url,
      workspaceId: options.workspaceId,
      readKey: "",
      // 回答のキー（NUXT_CMS_ANSWER_KEY）。非公開の設定だけに入る
      answerKey: "",
      siteUrl,
      webhookSecret: "",
      correctionFeeds: feeds,
      ai,
      search: { perMinute, ipHeader },
      answer: { ipHeader: answerIpHeader },
    };
    const previewRoute = options.preview ? (options.preview.route ?? "/_cms/preview") : "";
    if (previewRoute && (!url || !previewRoute.startsWith("/") || previewRoute.startsWith("//"))) {
      throw new RegisterError("cms.preview: set cms.url, and make cms.preview.route a path that starts with /");
    }
    nuxt.options.runtimeConfig.public.cms = { routePrefix: options.routePrefix, cmsOrigin: previewRoute ? new URL(url).origin : "", searchRoute, answerRoute };

    // クエリの登録（nuxt prepare では行わない）
    const queries = readQueries(resolve(nuxt.options.rootDir, options.queriesDir));
    for (const name of RESERVED_QUERIES) {
      if (Object.hasOwn(queries, name)) throw new RegisterError(`${options.queriesDir}/${name}.graphql: this name is used by the module`);
    }
    // リダイレクト一覧と訂正の一覧のクエリは、モジュールが自分で登録する
    if (siteUrl) queries[REDIRECTS_QUERY] = REDIRECTS_QUERY_TEXT;
    if (options.corrections) queries[CORRECTIONS_QUERY] = CORRECTIONS_QUERY_TEXT;
    if (searchRoute) queries[SEARCH_QUERY] = SEARCH_QUERY_TEXT;
    if (answerRoute) queries[ANSWER_QUERY] = ANSWER_QUERY_TEXT;
    if (ai) {
      queries[AI_CATALOG_QUERY] = AI_CATALOG_QUERY_TEXT;
      queries[AI_MARKDOWN_QUERY] = AI_MARKDOWN_QUERY_TEXT;
      if (ai.robotsTxt) queries[AI_OPT_OUTS_QUERY] = AI_OPT_OUTS_QUERY_TEXT;
    }
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
    // AI向けの素材: サーバーのルートで返す。nuxt generate ではビルドのときに書き出す（Q145）
    if (ai) {
      const prerender: string[] = [];
      if (ai.markdown) {
        addServerHandler({ middleware: true, handler: resolver.resolve("./runtime/server/ai-markdown.js") });
        // 記事の .md は、ビルドのときの目録から書き出す（読むキーがあるときだけ）
        const readKey = process.env["NUXT_CMS_READ_KEY"];
        if (nuxt.options.nitro.static && ids[AI_CATALOG_QUERY]) {
          if (readKey) {
            for (const { locale } of ai.locales) {
              const items = await loadCatalog({ url, workspaceId: options.workspaceId, readKey }, ids[AI_CATALOG_QUERY], locale);
              for (const item of items) {
                const md = markdownPath(item.url, ai.siteOrigin);
                if (md) prerender.push(md);
              }
            }
            logger.info(`writing ${prerender.length} Markdown version(s) of articles`);
          } else {
            logger.warn("NUXT_CMS_READ_KEY is not set at build time; the Markdown versions of articles are not written");
          }
        }
      }
      if (ai.llmsTxt) {
        addServerHandler({ route: "/llms.txt", method: "get", handler: resolver.resolve("./runtime/server/ai-llms.js") });
        prerender.push("/llms.txt");
      }
      if (ai.robotsTxt) {
        addServerHandler({ route: "/robots.txt", method: "get", handler: resolver.resolve("./runtime/server/ai-robots.js") });
        prerender.push("/robots.txt");
      }
      if (prerender.length > 0 && nuxt.options.nitro.static) {
        nuxt.options.nitro.prerender = { ...nuxt.options.nitro.prerender, routes: [...(nuxt.options.nitro.prerender?.routes ?? []), ...prerender] };
      }
    }
    addServerHandler({ route: options.webhookRoute, method: "post", handler: resolver.resolve("./runtime/server/webhook.js") });
    if (previewRoute) {
      for (const route of [previewRoute, `${previewRoute}/exit`]) {
        addServerHandler({ route, method: "get", handler: resolver.resolve("./runtime/server/preview.js") });
      }
      addPlugin({ src: resolver.resolve("./runtime/live-preview.js"), mode: "client" });
    }

    if (searchRoute) addServerHandler({ route: searchRoute, method: "get", handler: resolver.resolve("./runtime/server/search.js") });
    if (answerRoute) addServerHandler({ route: answerRoute, method: "post", handler: resolver.resolve("./runtime/server/answer.js") });
    addServerHandler({ route: `${options.routePrefix}/:name`, method: "get", handler: resolver.resolve("./runtime/server/query.js") });
    addImports([
      { name: "useCmsQuery", from: resolver.resolve("./runtime/composables.js") },
      { name: "useCmsCorrections", from: resolver.resolve("./runtime/composables.js") },
      { name: "useCmsAiMeta", from: resolver.resolve("./runtime/composables.js") },
      { name: "useCmsSearch", from: resolver.resolve("./runtime/composables.js") },
      { name: "cmsHighlight", from: resolver.resolve("./runtime/search.js") },
      { name: "useCmsAnswer", from: resolver.resolve("./runtime/composables.js") },
    ]);
    addComponent({ name: "CmsBody", filePath: resolver.resolve("./runtime/body.js") });
    addComponent({ name: "CmsCorrections", filePath: resolver.resolve("./runtime/corrections.js") });
    addComponent({ name: "CmsDeletedAnchorNotice", filePath: resolver.resolve("./runtime/deleted-anchor.js") });
    if (searchRoute) addComponent({ name: "CmsSearchBox", filePath: resolver.resolve("./runtime/search-box.js") });
    addComponent({ name: "CmsAnswerText", filePath: resolver.resolve("./runtime/answer-text.js") });
  },
});
