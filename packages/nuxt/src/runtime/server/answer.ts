// サイトのサーバーのルート `POST <answer.route>`（本文は JSON の `{ question, locale }`）。チャットボットの回答の中継。
// CMS の判定先行のストリーミングを、SSE のままブラウザに渡す（イベントの形は ../answer.ts の CmsAnswerEvent）。
// 中身は answer-core.ts（cached は渡さない。エラーはコードだけ）。

import { defineEventHandler, getRequestHeaders, getRequestIP, readBody, sendStream, setResponseHeader, setResponseStatus } from "h3";
import { useRuntimeConfig } from "nitropack/runtime";
// ビルドのときに作る、クエリの名前 → 登録したID の対応（モジュールが Nitro の仮想モジュールとして渡す）
import queries from "#cms/queries";

import { AnswerRelayError, openAnswer, parseAnswerInput, toSse, visitorIdOf } from "./answer-core.ts";
import { ANSWER_QUERY } from "./names.ts";
import { clientIp } from "./search-core.ts";

export default defineEventHandler(async (event) => {
  const config = useRuntimeConfig(event) as unknown as {
    cms: { url: string; workspaceId: string; answerKey: string; answer: { ipHeader: string } };
  };
  // 回答は訪問者ごとに違い、お金もかかるので、どこにもキャッシュさせない
  setResponseHeader(event, "Cache-Control", "no-store");
  const fail = (e: AnswerRelayError) => {
    setResponseStatus(event, e.status);
    if (e.retryAfter !== undefined) setResponseHeader(event, "Retry-After", e.retryAfter);
    return { code: e.code };
  };
  const id = queries[ANSWER_QUERY];
  if (!id || !config.cms.answerKey) return fail(new AnswerRelayError(500, "ANSWER_FAILED"));
  const abort = new AbortController();
  // 訪問者が接続を切ったら、CMS への接続も切る（LLM の応答を読むのをやめる）
  event.node.res.on("close", () => abort.abort());
  let events: AsyncGenerator<import("../answer.ts").CmsAnswerEvent>;
  try {
    const input = parseAnswerInput(await readBody(event));
    const ip = clientIp(getRequestHeaders(event), getRequestIP(event), config.cms.answer.ipHeader);
    events = await openAnswer(config.cms, id, { ...input, visitorId: visitorIdOf(ip, config.cms.answerKey) }, abort.signal);
  } catch (e) {
    if (e instanceof AnswerRelayError) return fail(e);
    throw e;
  }
  setResponseHeader(event, "Content-Type", "text/event-stream; charset=utf-8");
  setResponseHeader(event, "X-Accel-Buffering", "no");
  const encoder = new TextEncoder();
  const stream = new ReadableStream<Uint8Array>({
    async pull(controller) {
      const next = await events.next();
      if (next.done) controller.close();
      else controller.enqueue(encoder.encode(toSse(next.value)));
    },
    async cancel() {
      abort.abort();
      await events.return(undefined);
    },
  });
  return sendStream(event, stream);
});
