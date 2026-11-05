// サイトのサーバーの、回答の中継のルートの中身（Nitro に依存しない。テストで直接呼ぶ）。09-qa-chatbot.md「呼び出し経路」。
// - ブラウザから CMS を直接呼ばせない（原則1）。回答のキー（answer の範囲）はサイトのサーバーだけが持つ
// - 訪問者の識別子は、訪問者のIPアドレスを回答のキーで HMAC-SHA256 にかけたもの（IPアドレスそのものは CMS に送らない）。
//   CMS が訪問者ごとの回数を数える（サイトでは数えない）
// - CMS の判定先行のストリーミング（GraphQL over SSE）を読み、決まった項目だけをブラウザに渡す。
//   **回答キャッシュから返したか（cached）は渡さない**（クエリでも選ばない）。エラーはコードだけを渡す

import { createHmac } from "node:crypto";

import {
  type CmsAnswer,
  type CmsAnswerErrorCode,
  type CmsAnswerEvent,
  type CmsAnswerSource,
  MAX_QUESTION_CHARS,
  PASSED_ERROR_CODES,
  SseReader,
} from "../answer.ts";

/** CMS を呼ぶ先。 */
export interface AnswerTarget {
  url: string;
  workspaceId: string;
  /** 回答のキー（answer の範囲）。 */
  answerKey: string;
}

/** ストリームが始まる前に断ったとき（ルートは status で返し、本文はコードだけ）。 */
export class AnswerRelayError extends Error {
  constructor(
    readonly status: number,
    readonly code: CmsAnswerErrorCode,
    readonly retryAfter?: number,
  ) {
    super(code);
  }
}

/** ロケールの形（BCP 47 のおおまかな形）。 */
const LOCALE = /^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$/;
/** 回答を待つ時間の上限（CMS の上限 90 秒より少し長く）。 */
const TIMEOUT_MS = 120_000;

/** ルートの本文（JSON の `{ question, locale }`）を確かめる。 */
export function parseAnswerInput(body: unknown): { question: string; locale: string } {
  const b = (body ?? {}) as { question?: unknown; locale?: unknown };
  if (typeof b.question !== "string" || typeof b.locale !== "string" || !LOCALE.test(b.locale)) {
    throw new AnswerRelayError(400, "ANSWER_FAILED");
  }
  const question = b.question.trim();
  if (!question) throw new AnswerRelayError(400, "QUESTION_EMPTY");
  if ([...question].length > MAX_QUESTION_CHARS) throw new AnswerRelayError(400, "QUESTION_TOO_LONG");
  return { question, locale: b.locale };
}

/** 訪問者の識別子（IPアドレスを回答のキーで HMAC-SHA256 にかけた16進）。 */
export function visitorIdOf(ip: string, answerKey: string): string {
  return createHmac("sha256", answerKey).update(ip).digest("hex");
}

/** 出典の決まった項目だけを写す。 */
function source(raw: unknown): CmsAnswerSource {
  const s = (raw ?? {}) as Record<string, unknown>;
  const str = (v: unknown) => (typeof v === "string" ? v : null);
  return {
    citation: typeof s["citation"] === "number" ? s["citation"] : 0,
    entryId: String(s["entryId"] ?? ""),
    locale: String(s["locale"] ?? ""),
    url: str(s["url"]),
    anchor: str(s["anchor"]),
    breadcrumb: Array.isArray(s["breadcrumb"]) ? s["breadcrumb"].filter((x): x is string => typeof x === "string") : [],
    versionId: String(s["versionId"] ?? ""),
    lastReviewedAt: str(s["lastReviewedAt"]),
    reviewExpired: s["reviewExpired"] === true,
  };
}
const sources = (raw: unknown): CmsAnswerSource[] => (Array.isArray(raw) ? raw.map(source) : []);

/** CMS のエラーのコードを、ブラウザに渡すコードにする。 */
export function errorCode(code: unknown): CmsAnswerErrorCode {
  return PASSED_ERROR_CODES.find((c) => c === code) ?? "ANSWER_FAILED";
}

/** GraphQL の応答（`next` のイベントの data）を、ブラウザに渡すイベントにする。知らない形なら null。 */
export function toEvent(payload: unknown): CmsAnswerEvent | null {
  const p = (payload ?? {}) as { data?: { answerStream?: Record<string, unknown> | null } | null; errors?: unknown };
  if (Array.isArray(p.errors) && p.errors.length > 0) {
    const first = p.errors[0] as { extensions?: { code?: unknown } } | undefined;
    return { type: "error", code: errorCode(first?.extensions?.code) };
  }
  const e = p.data?.answerStream;
  if (!e) return null;
  switch (e["__typename"]) {
    case "CmsAnswerVerdict":
      return { type: "verdict", answerable: e["answerable"] === true, sources: sources(e["sources"]) };
    case "CmsAnswerDelta":
      return typeof e["text"] === "string" ? { type: "delta", text: e["text"] } : null;
    case "CmsAnswerFinal": {
      const answer: CmsAnswer = {
        status: (["ANSWERED", "NO_INFORMATION", "VIOLATION_DETECTED"] as const).find((s) => s === e["status"]) ?? "NO_INFORMATION",
        text: typeof e["text"] === "string" ? e["text"] : "",
        truncated: e["truncated"] === true,
        sources: sources(e["sources"]),
      };
      return { type: "final", answer };
    }
    default:
      return null;
  }
}

/** ブラウザに送る SSE の1つのイベント（type 以外の項目を data に）。 */
export function toSse(event: CmsAnswerEvent): string {
  const { type, ...data } = event;
  return `event: ${type}\ndata: ${JSON.stringify(data)}\n\n`;
}

/**
 * CMS の回答のストリーミングを始める。始まる前に断られたら AnswerRelayError。始まったら、イベントを順に返す
 * （final か error で終わる。signal を中止すると、CMS への接続も切る）。
 */
export async function openAnswer(
  target: AnswerTarget,
  queryId: string,
  input: { question: string; locale: string; visitorId: string },
  signal: AbortSignal,
  fetchImpl: typeof fetch = fetch,
): Promise<AsyncGenerator<CmsAnswerEvent>> {
  const u = `${target.url.replace(/\/+$/, "")}/ws/${encodeURIComponent(target.workspaceId)}/graphql`;
  let res: Response;
  try {
    res = await fetchImpl(u, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${target.answerKey}`,
        Accept: "text/event-stream",
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ id: queryId, variables: input }),
      redirect: "error",
      signal: AbortSignal.any([signal, AbortSignal.timeout(TIMEOUT_MS)]),
    });
  } catch {
    throw new AnswerRelayError(502, "ANSWER_FAILED");
  }
  if (res.status === 429) {
    const after = Number(res.headers.get("retry-after"));
    throw new AnswerRelayError(429, "RATE_LIMITED", Number.isFinite(after) && after > 0 ? after : undefined);
  }
  // キーの誤りなど、CMS が断った理由は訪問者に知らせない（サイトの設定の誤り）
  if (!res.ok || !res.body) throw new AnswerRelayError(502, "ANSWER_FAILED");
  if (!(res.headers.get("content-type") ?? "").startsWith("text/event-stream")) {
    // ストリームの前のエラー（GraphQL の応答）
    let body: unknown = null;
    try {
      body = await res.json();
    } catch {
      body = null;
    }
    const e = toEvent(body);
    throw new AnswerRelayError(e?.type === "error" && e.code !== "ANSWER_FAILED" ? 400 : 502, e?.type === "error" ? e.code : "ANSWER_FAILED");
  }
  const reader = res.body.getReader();
  return (async function* () {
    const sse = new SseReader();
    const decoder = new TextDecoder();
    try {
      for (;;) {
        let chunk: ReadableStreamReadResult<Uint8Array>;
        try {
          chunk = await reader.read();
        } catch {
          yield { type: "error", code: "ANSWER_FAILED" };
          return;
        }
        if (chunk.done) {
          // 最終判定の前に切れた
          yield { type: "error", code: "ANSWER_FAILED" };
          return;
        }
        for (const { event, data } of sse.push(decoder.decode(chunk.value, { stream: true }))) {
          if (event === "complete") return;
          if (event !== "next") continue;
          let payload: unknown;
          try {
            payload = JSON.parse(data);
          } catch {
            yield { type: "error", code: "ANSWER_FAILED" };
            return;
          }
          const e = toEvent(payload);
          if (!e) continue;
          yield e;
          if (e.type === "final" || e.type === "error") return;
        }
      }
    } finally {
      await reader.cancel().catch(() => undefined);
    }
  })();
}
