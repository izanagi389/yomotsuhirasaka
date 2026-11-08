// サイトのサーバーの、CMSからのWebhookの受け口（Q145, Q146）。署名を確かめてから、CMSから取ったもののキャッシュを消す。
// - 秘密は NUXT_YOMI_WEBHOOK_SECRET（非公開の設定）。入れ替え中は、カンマで区切って新旧の両方を書く
// - 秘密が設定されていなければ、受け口はないものとして 404 を返す
// - 署名が正しくない・古い・同じイベントの再送のときは 401。理由は返さない
// - 本文の中身は使わない（どの記事が変わっても、キャッシュとサイト内検索の結果をまとめて消す）。
//   関連記事の一覧が変わった知らせ（entries.related_changed）も同じように扱う（モジュールは関連記事を覚えていない）

import { createError, defineEventHandler, getHeader, readRawBody, setResponseStatus } from "h3";
import { useRuntimeConfig } from "nitropack/runtime";

import { clearAll } from "./cache.ts";
import { clearSearchCache } from "./search-core.ts";
import { MAX_BODY_BYTES, SeenEvents, verifySignature } from "./webhook-verify.ts";

const seen = new SeenEvents();

export default defineEventHandler(async (event) => {
  const config = useRuntimeConfig(event) as unknown as { yomi: { webhookSecret: string } };
  const secrets = (config.yomi.webhookSecret ?? "").split(",").map((s) => s.trim()).filter(Boolean);
  if (secrets.length === 0) throw createError({ statusCode: 404, statusMessage: "Not Found" });
  const length = Number(getHeader(event, "content-length") ?? "0");
  if (!Number.isFinite(length) || length > MAX_BODY_BYTES) throw createError({ statusCode: 413, statusMessage: "Payload Too Large" });
  const body = (await readRawBody(event, "utf8")) ?? "";
  if (!verifySignature(getHeader(event, "webhook-signature"), body, secrets, seen)) {
    throw createError({ statusCode: 401, statusMessage: "Unauthorized" });
  }
  clearAll();
  clearSearchCache();
  setResponseStatus(event, 204);
  return null;
});
