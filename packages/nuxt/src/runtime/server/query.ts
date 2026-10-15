// サイトのサーバーのルート `<routePrefix>/<クエリの名前>?v=<変数のJSON>`。
// ブラウザでの画面の移動のときも、ここを通して取る（ブラウザから配信APIを直接呼ばせない。原則1）。
// 呼べるのは、ビルドのときに登録したクエリだけ。

import { createError, defineEventHandler, getQuery, getRouterParam } from "h3";
import { useRuntimeConfig } from "nitropack/runtime";
// ビルドのときに作る、クエリの名前 → 登録したID の対応（モジュールが Nitro の仮想モジュールとして渡す）
import queries from "#cms/queries";

import { QueryError, fetchQuery, parseVariables } from "./fetch-query.ts";

export default defineEventHandler(async (event) => {
  const name = getRouterParam(event, "name") ?? "";
  const id = Object.hasOwn(queries, name) ? queries[name] : undefined;
  if (!id) throw createError({ statusCode: 404, statusMessage: "Unknown query" });
  const config = useRuntimeConfig(event) as unknown as { cms: { url: string; workspaceId: string; readKey: string } };
  if (!config.cms.readKey) throw createError({ statusCode: 500, statusMessage: "The CMS read key is not configured" });
  try {
    const variables = parseVariables(getQuery(event)["v"]);
    return await fetchQuery(config.cms, id, variables);
  } catch (e) {
    if (e instanceof QueryError) throw createError({ statusCode: e.status, statusMessage: e.message });
    throw e;
  }
});
