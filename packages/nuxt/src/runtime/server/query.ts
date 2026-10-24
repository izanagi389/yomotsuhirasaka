// サイトのサーバーのルート `<routePrefix>/<クエリの名前>?v=<変数のJSON>`。
// ブラウザでの画面の移動のときも、ここを通して取る（ブラウザから配信APIを直接呼ばせない。原則1）。
// 呼べるのは、ビルドのときに登録したクエリだけ。
// プレビュー中（プレビュー用のクッキーがあるとき）は、トークンを付けて取り、応答をキャッシュさせない。
// トークンが切れていたら、クッキーを消して公開中の内容を返す。

import { createError, defineEventHandler, getQuery, getRouterParam, setResponseHeader } from "h3";
import { useRuntimeConfig } from "nitropack/runtime";
// ビルドのときに作る、クエリの名前 → 登録したID の対応（モジュールが Nitro の仮想モジュールとして渡す）
import queries from "#cms/queries";

import { QueryError, fetchQuery, parseVariables } from "./fetch-query.ts";
import { clearPreviewCookie, previewTokenOf } from "./preview-cookie.ts";

export default defineEventHandler(async (event) => {
  const name = getRouterParam(event, "name") ?? "";
  const id = Object.hasOwn(queries, name) ? queries[name] : undefined;
  if (!id) throw createError({ statusCode: 404, statusMessage: "Unknown query" });
  const config = useRuntimeConfig(event) as unknown as { cms: { url: string; workspaceId: string; readKey: string } };
  if (!config.cms.readKey) throw createError({ statusCode: 500, statusMessage: "The CMS read key is not configured" });
  try {
    const variables = parseVariables(getQuery(event)["v"]);
    const previewToken = previewTokenOf(event);
    if (!previewToken) return await fetchQuery(config.cms, id, variables);
    setResponseHeader(event, "Cache-Control", "private, no-store");
    try {
      return await fetchQuery({ ...config.cms, previewToken }, id, variables);
    } catch (e) {
      if (!(e instanceof QueryError) || e.code !== "invalid_preview_token") throw e;
      clearPreviewCookie(event);
      return await fetchQuery(config.cms, id, variables);
    }
  } catch (e) {
    if (e instanceof QueryError) throw createError({ statusCode: e.status, statusMessage: e.message });
    throw e;
  }
});
