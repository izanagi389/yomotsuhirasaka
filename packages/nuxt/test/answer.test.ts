import { describe, expect, it, vi } from "vitest";
import { effectScope, h } from "vue";
import { renderToString } from "vue/server-renderer";

import { type CmsAnswerEvent, type CmsAnswerSource, SseReader, parseAnswerMarkdown, sourceHref } from "../src/runtime/answer.ts";
import { AnswerRelayError, openAnswer, parseAnswerInput, toEvent, toSse, visitorIdOf } from "../src/runtime/server/answer-core.ts";
import { ANSWER_QUERY_TEXT } from "../src/runtime/server/names.ts";

vi.mock("nuxt/app", () => ({
  useRuntimeConfig: () => ({ public: { cms: { answerRoute: "/_cms/answer" } } }),
  useFetch: vi.fn(),
  useHead: vi.fn(),
}));

const { useCmsAnswer } = await import("../src/runtime/composables.ts");
const { default: CmsAnswerText } = await import("../src/runtime/answer-text.ts");

const source = (over: Partial<CmsAnswerSource> = {}): CmsAnswerSource => ({
  citation: 1,
  entryId: "e-1",
  locale: "ja",
  url: "https://example.com/posts/a",
  anchor: "h-1",
  breadcrumb: ["記事A"],
  versionId: "v-1",
  lastReviewedAt: null,
  reviewExpired: false,
  ...over,
});

const target = { url: "https://cms.example.com/", workspaceId: "ws-1", answerKey: "cms_key_secret" };
const input = { question: "料金は？", locale: "ja", visitorId: "v" };

/** CMS の GraphQL over SSE の本文（`next` に GraphQL の応答、最後に `complete`）。 */
const cmsSse = (payloads: unknown[], complete = true): string =>
  payloads.map((p) => `event: next\ndata: ${JSON.stringify(p)}\n\n`).join("") + (complete ? "event: complete\ndata:\n\n" : "");
const next = (answerStream: Record<string, unknown>) => ({ data: { answerStream } });

/** 本文を小さな断片に分けて返す、偽の CMS（呼ばれた依頼も記録する）。 */
function fakeCms(status: number, body: string, headers: Record<string, string> = { "content-type": "text/event-stream" }) {
  const calls: { url: string; init: RequestInit }[] = [];
  const impl = (async (url: string | URL, init?: RequestInit) => {
    calls.push({ url: String(url), init: init ?? {} });
    const bytes = new TextEncoder().encode(body);
    const stream = new ReadableStream<Uint8Array>({
      start(c) {
        for (let i = 0; i < bytes.length; i += 7) c.enqueue(bytes.slice(i, i + 7));
        c.close();
      },
    });
    return new Response(stream, { status, headers });
  }) as typeof fetch;
  return { impl, calls };
}

async function collect(gen: AsyncGenerator<CmsAnswerEvent>): Promise<CmsAnswerEvent[]> {
  const out: CmsAnswerEvent[] = [];
  for await (const e of gen) out.push(e);
  return out;
}

describe("parseAnswerInput", () => {
  it("質問とロケールを確かめる", () => {
    expect(parseAnswerInput({ question: "  料金は？ ", locale: "ja" })).toEqual({ question: "料金は？", locale: "ja" });
    const code = (b: unknown) => {
      try {
        parseAnswerInput(b);
        return null;
      } catch (e) {
        return (e as AnswerRelayError).code;
      }
    };
    expect(code({ question: "  ", locale: "ja" })).toBe("QUESTION_EMPTY");
    expect(code({ question: "あ".repeat(501), locale: "ja" })).toBe("QUESTION_TOO_LONG");
    expect(code({ question: "あ".repeat(500), locale: "ja" })).toBeNull();
    expect(code({ question: "q", locale: "../x" })).toBe("ANSWER_FAILED");
    expect(code(null)).toBe("ANSWER_FAILED");
  });
});

describe("visitorIdOf", () => {
  it("IPアドレスをそのまま含まず、キーごとに違う", () => {
    const a = visitorIdOf("203.0.113.5", "k1");
    expect(a).toMatch(/^[0-9a-f]{64}$/);
    expect(a).not.toContain("203.0.113.5");
    expect(visitorIdOf("203.0.113.5", "k1")).toBe(a);
    expect(visitorIdOf("203.0.113.5", "k2")).not.toBe(a);
    expect(visitorIdOf("203.0.113.6", "k1")).not.toBe(a);
  });
});

describe("openAnswer", () => {
  it("回答キャッシュから返したか（cached）は渡さず、決まった項目だけを順に渡す", async () => {
    const body = cmsSse([
      next({ __typename: "CmsAnswerVerdict", answerable: true, sources: [{ ...source(), extra: "x" }] }),
      next({ __typename: "CmsAnswerDelta", text: "月額です" }),
      next({ __typename: "CmsAnswerDelta", text: "[1]" }),
      next({ __typename: "CmsAnswerFinal", status: "ANSWERED", text: "月額です[1]", truncated: false, cached: true, sources: [source()] }),
    ]);
    const { impl, calls } = fakeCms(200, body);
    const events = await collect(await openAnswer(target, "q-1", input, new AbortController().signal, impl));
    expect(events.map((e) => e.type)).toEqual(["verdict", "delta", "delta", "final"]);
    expect(JSON.stringify(events)).not.toContain("cached");
    expect(JSON.stringify(events)).not.toContain("extra");
    expect(events.map(toSse).join("")).not.toContain("cached");
    expect(events[3]).toEqual({ type: "final", answer: { status: "ANSWERED", text: "月額です[1]", truncated: false, sources: [source()] } });
    // CMS への依頼: POST、回答のキー、登録したクエリのIDと変数
    expect(calls[0]!.url).toBe("https://cms.example.com/ws/ws-1/graphql");
    expect(calls[0]!.init.method).toBe("POST");
    expect((calls[0]!.init.headers as Record<string, string>)["Authorization"]).toBe("Bearer cms_key_secret");
    expect(JSON.parse(String(calls[0]!.init.body))).toEqual({ id: "q-1", variables: input });
  });

  it("クエリでも cached を選ばない", () => {
    expect(ANSWER_QUERY_TEXT).not.toContain("cached");
    expect(ANSWER_QUERY_TEXT).toContain("citation");
  });

  it("エラーはコードだけ。知らないコードは ANSWER_FAILED", async () => {
    const err = (code: string, message = "secret detail") => ({ errors: [{ message, extensions: { code } }] });
    const run = async (payload: unknown) => collect(await openAnswer(target, "q", input, new AbortController().signal, fakeCms(200, cmsSse([payload])).impl));
    expect(await run(err("ANSWER_BUDGET_EXHAUSTED"))).toEqual([{ type: "error", code: "ANSWER_BUDGET_EXHAUSTED" }]);
    expect(await run(err("QUESTION_REFUSED"))).toEqual([{ type: "error", code: "QUESTION_REFUSED" }]);
    const internal = await run(err("INTERNAL_ERROR"));
    expect(internal).toEqual([{ type: "error", code: "ANSWER_FAILED" }]);
    expect(JSON.stringify(internal)).not.toContain("secret detail");
  });

  it("最終判定の前に切れたら ANSWER_FAILED で終わる", async () => {
    const body = cmsSse([next({ __typename: "CmsAnswerDelta", text: "途中" })], false);
    const events = await collect(await openAnswer(target, "q", input, new AbortController().signal, fakeCms(200, body).impl));
    expect(events).toEqual([{ type: "delta", text: "途中" }, { type: "error", code: "ANSWER_FAILED" }]);
  });

  it("回数の制限は 429 と Retry-After、そのほかの断りは 502（理由は渡さない）", async () => {
    const reject = async (status: number, headers: Record<string, string>, body = "{}") => {
      try {
        await openAnswer(target, "q", input, new AbortController().signal, fakeCms(status, body, headers).impl);
        return null;
      } catch (e) {
        return e as AnswerRelayError;
      }
    };
    const limited = await reject(429, { "content-type": "application/json", "retry-after": "30" }, '{"error":"visitor_rate_limited"}');
    expect([limited?.status, limited?.code, limited?.retryAfter]).toEqual([429, "RATE_LIMITED", 30]);
    const unauthorized = await reject(401, { "content-type": "application/json" }, '{"error":"invalid_api_key"}');
    expect([unauthorized?.status, unauthorized?.code]).toEqual([502, "ANSWER_FAILED"]);
    // ストリームの前の GraphQL のエラー
    const early = await reject(200, { "content-type": "application/json" }, JSON.stringify({ errors: [{ extensions: { code: "LOCALE_NOT_SUPPORTED" } }] }));
    expect([early?.status, early?.code]).toEqual([400, "LOCALE_NOT_SUPPORTED"]);
  });

  it("知らない形の応答は無視する", () => {
    expect(toEvent({ data: { answerStream: { __typename: "Other" } } })).toBeNull();
    expect(toEvent(null)).toBeNull();
  });
});

describe("回答の本文（Markdown の一部）", () => {
  const render = (text: string, sources: CmsAnswerSource[] = [source()]) => renderToString(h(CmsAnswerText, { text, sources }));

  it("文字はエスケープし、HTML は描かない", async () => {
    const html = await render('<script>alert(1)</script> & <img src=x onerror="a()">');
    expect(html).not.toContain("<script");
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;script&gt;");
  });

  it("画像は描かず、描いてよくない URL はリンクにしない", async () => {
    const html = await render("![図](https://example.com/a.png) [危険](javascript:alert(1)) [相対](/a) [良い](https://example.com/b)");
    expect(html).not.toContain("a.png");
    expect(html).not.toContain("図");
    expect(html).not.toContain("javascript:");
    expect(html).toContain("危険");
    expect(html).not.toContain('href="/a"');
    expect(html).toContain('<a href="https://example.com/b" rel="noopener noreferrer nofollow">良い</a>');
  });

  it("[n] は citation が同じ出典のセクションへのリンク。出典がなければ文字のまま", async () => {
    const html = await render("A[3]、B[1]、C[9]", [source({ citation: 3, url: "https://example.com/posts/c", anchor: "料金" })]);
    expect(html).toContain(`<sup><a href="https://example.com/posts/c#${encodeURIComponent("料金")}">[3]</a></sup>`);
    expect(html).toContain("B[1]");
    expect(html).toContain("C[9]");
  });

  it("段落・箇条書き・太字・コード。見出しは太字の段落", () => {
    expect(parseAnswerMarkdown("## 見出し\n\n- **a**\n- `b`\n\n1. x\n\n本文\n続き")).toEqual([
      { type: "paragraph", children: [{ type: "strong", children: [{ type: "text", text: "見出し" }] }] },
      { type: "list", ordered: false, items: [[{ type: "strong", children: [{ type: "text", text: "a" }] }], [{ type: "code", text: "b" }]] },
      { type: "list", ordered: true, items: [[{ type: "text", text: "x" }]] },
      { type: "paragraph", children: [{ type: "text", text: "本文\n続き" }] },
    ]);
    expect(sourceHref(source({ url: "javascript:alert(1)" }))).toBeNull();
  });
});

describe("SseReader", () => {
  it("区切りの途中で切れた本文もつなぐ", () => {
    const r = new SseReader();
    expect(r.push("event: delta\nda")).toEqual([]);
    expect(r.push('ta: {"text":"a"}\n\nevent: final\n')).toEqual([{ event: "delta", data: '{"text":"a"}' }]);
  });
});

describe("useCmsAnswer", () => {
  it("サイトのサーバーのルートに送り、届いた順に状態を変える", async () => {
    const sse =
      toSse({ type: "verdict", answerable: true, sources: [source()] }) +
      toSse({ type: "delta", text: "月額" }) +
      toSse({ type: "final", answer: { status: "ANSWERED", text: "月額です[1]", truncated: false, sources: [source()] } });
    const fetchMock = vi.fn(async () => new Response(sse, { status: 200, headers: { "content-type": "text/event-stream" } }));
    vi.stubGlobal("fetch", fetchMock);
    const scope = effectScope();
    const { state, ask } = scope.run(() => useCmsAnswer())!;
    await ask("料金は？", "ja");
    expect(fetchMock).toHaveBeenCalledWith("/_cms/answer", expect.objectContaining({ method: "POST", body: JSON.stringify({ question: "料金は？", locale: "ja" }) }));
    expect(state.value.phase).toBe("done");
    expect(state.value.text).toBe("月額です[1]");
    expect(state.value.answer?.status).toBe("ANSWERED");
    scope.stop();
    vi.unstubAllGlobals();
  });

  it("断られたらコードと Retry-After を状態に入れる", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response('{"code":"RATE_LIMITED"}', { status: 429, headers: { "content-type": "application/json", "retry-after": "20" } })),
    );
    const scope = effectScope();
    const { state, ask } = scope.run(() => useCmsAnswer())!;
    await ask("q", "ja");
    expect(state.value.phase).toBe("error");
    expect(state.value.error).toEqual({ code: "RATE_LIMITED", retryAfter: 20 });
    scope.stop();
    vi.unstubAllGlobals();
  });
});
