import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { startMockLlm, type MockLlm } from "../src/server.js";
import { MAX_RECORDED } from "../src/state.js";

let mock: MockLlm;
beforeAll(async () => {
  mock = await startMockLlm();
});
afterAll(async () => {
  await mock.close();
});

let keySeq = 0;
const newKey = () => `test-key-${++keySeq}`;

async function call(key: string | null, method: string, path: string, body?: unknown) {
  const res = await fetch(`${mock.url}${path}`, {
    method,
    headers: { ...(key ? { authorization: `Bearer ${key}` } : {}), ...(body !== undefined ? { "content-type": "application/json" } : {}) },
    ...(body !== undefined ? { body: JSON.stringify(body) } : {}),
  });
  return { status: res.status, headers: res.headers, text: await res.text() };
}

const embeddings = (key: string | null, body: unknown = { model: "mock-embed-8", input: "こんにちは世界" }) =>
  call(key, "POST", "/v1/embeddings", body);
const chatBody = (extra: Record<string, unknown> = {}) => ({
  model: "mock-chat",
  messages: [
    { role: "system", content: "[1] pg_bigm で日本語の全文検索ができます。\n[2] カレーは玉ねぎを炒めます。" },
    { role: "user", content: "日本語の全文検索は？" },
  ],
  ...extra,
});
const chat = (key: string, extra: Record<string, unknown> = {}) => call(key, "POST", "/v1/chat/completions", chatBody(extra));
const setFaults = (key: string, faults: unknown[]) => call(key, "PUT", "/_mock/faults", { faults });
const requests = async (key: string) => JSON.parse((await call(key, "GET", "/_mock/requests")).text).requests as { status: number; faults: string[]; body: unknown }[];

function sseData(text: string): string[] {
  return text
    .split("\n\n")
    .filter((b) => b.startsWith("data: "))
    .map((b) => b.slice("data: ".length));
}

describe("API", () => {
  test("キーがなければ 401、知らないモデル名は 404、知らないパスは 404", async () => {
    expect((await embeddings(null)).status).toBe(401);
    expect((await call("", "POST", "/_mock/faults", { faults: [] })).status).toBe(401);
    expect((await embeddings(newKey(), { model: "text-embedding-3-small", input: "x" })).status).toBe(404);
    expect((await call(newKey(), "POST", "/v1/chat/completions", chatBody({ model: "gpt-4o" }))).status).toBe(404);
    expect((await call(newKey(), "GET", "/v1/models")).status).toBe(404);
    expect((await embeddings(newKey(), { model: "mock-embed-8", input: "" })).status).toBe(400);
  });

  test("埋め込み: 次元数・並び順・usage", async () => {
    const res = await embeddings(newKey(), { model: "mock-embed-16", input: ["abcde", "日本語"] });
    expect(res.status).toBe(200);
    const json = JSON.parse(res.text);
    expect(json.data.map((d: { index: number; embedding: number[] }) => [d.index, d.embedding.length])).toEqual([
      [0, 16],
      [1, 16],
    ]);
    expect(json.usage).toEqual({ prompt_tokens: 3, total_tokens: 3 });
  });

  test("チャット（ストリーミングでない）", async () => {
    const res = await chat(newKey());
    expect(res.status).toBe(200);
    const json = JSON.parse(res.text);
    expect(json.choices[0].message.content).toBe("ANSWERABLE: yes; SOURCES: 1\npg_bigm で日本語の全文検索ができます。 [1]");
    expect(json.choices[0].finish_reason).toBe("stop");
    expect(json.usage.completion_tokens).toBe(Math.ceil([...json.choices[0].message.content].length / 4));
  });

  test("チャット（ストリーミング）: つなげると同じ回答になり、include_usage なら usage が来て、最後は [DONE]", async () => {
    const key = newKey();
    const plain = JSON.parse((await chat(key)).text).choices[0].message.content as string;
    const res = await chat(key, { stream: true, stream_options: { include_usage: true } });
    expect(res.headers.get("content-type")).toContain("text/event-stream");
    const data = sseData(res.text);
    expect(data.at(-1)).toBe("[DONE]");
    const chunks = data.slice(0, -1).map((d) => JSON.parse(d));
    expect(chunks.map((c) => c.choices[0]?.delta?.content ?? "").join("")).toBe(plain);
    expect(chunks.some((c) => c.choices[0]?.finish_reason === "stop")).toBe(true);
    expect(chunks.at(-1).usage.total_tokens).toBeGreaterThan(0);
  });
});

describe("障害と記録", () => {
  test("429 は Retry-After 付きで、count の回数だけ効く", async () => {
    const key = newKey();
    expect((await setFaults(key, [{ kind: "rateLimit", count: 2, retryAfterSeconds: 7 }])).status).toBe(204);
    const first = await embeddings(key);
    expect(first.status).toBe(429);
    expect(first.headers.get("retry-after")).toBe("7");
    expect((await embeddings(key)).status).toBe(429);
    expect((await embeddings(key)).status).toBe(200);
  });

  test("障害はキーごとに分かれ、消すまで続く", async () => {
    const a = newKey();
    const b = newKey();
    await setFaults(a, [{ kind: "serverError" }]);
    expect((await embeddings(a)).status).toBe(500);
    expect((await embeddings(b)).status).toBe(200);
    expect((await embeddings(a)).status).toBe(500);
    expect((await call(a, "DELETE", "/_mock/faults")).status).toBe(204);
    expect((await embeddings(a)).status).toBe(200);
  });

  test("endpoint で効く API を絞れる。チャットだけの障害を埋め込みに指定すると 400", async () => {
    const key = newKey();
    await setFaults(key, [{ kind: "serverError", endpoint: "chat" }]);
    expect((await embeddings(key)).status).toBe(200);
    expect((await chat(key)).status).toBe(500);
    expect((await setFaults(key, [{ kind: "cutStream", endpoint: "embeddings" }])).status).toBe(400);
    expect((await setFaults(key, [{ kind: "nope" }])).status).toBe(400);
  });

  test("タイムアウトは指定した時間だけ待ってから応答する", async () => {
    const key = newKey();
    await setFaults(key, [{ kind: "timeout", delayMs: 300, count: 1 }]);
    const started = Date.now();
    expect((await embeddings(key)).status).toBe(200);
    expect(Date.now() - started).toBeGreaterThanOrEqual(290);
  });

  test("壊れた JSON と usage なし", async () => {
    const key = newKey();
    await setFaults(key, [{ kind: "brokenJson", count: 1 }]);
    const broken = await embeddings(key);
    expect(broken.status).toBe(200);
    expect(() => JSON.parse(broken.text)).toThrow();
    await setFaults(key, [{ kind: "noUsage" }]);
    expect(JSON.parse((await embeddings(key)).text).usage).toBeUndefined();
    expect(JSON.parse((await chat(key)).text).usage).toBeUndefined();
  });

  test("途中で切れるストリームは、[DONE] も終わりの理由もなく切れる", async () => {
    const key = newKey();
    await setFaults(key, [{ kind: "cutStream" }]);
    const text = await fetch(`${mock.url}/v1/chat/completions`, {
      method: "POST",
      headers: { authorization: `Bearer ${key}`, "content-type": "application/json" },
      body: JSON.stringify(chatBody({ stream: true })),
    }).then(
      (r) => r.text().catch(() => "<cut>"),
      () => "<cut>",
    );
    expect(text).not.toContain("[DONE]");
    expect(text).not.toContain('"finish_reason":"stop"');
  });

  test("回答の組み立て方をキーごとに指定できる", async () => {
    const key = newKey();
    expect((await call(key, "PUT", "/_mock/strategy", { strategy: { kind: "fixed", content: "ANSWERABLE: no; SOURCES: none\nZ" } })).status).toBe(204);
    expect(JSON.parse((await chat(key)).text).choices[0].message.content).toBe("ANSWERABLE: no; SOURCES: none\nZ");
    expect(JSON.parse((await chat(newKey())).text).choices[0].message.content).toContain("SOURCES: 1");
    await call(key, "DELETE", "/_mock/strategy");
    expect(JSON.parse((await chat(key)).text).choices[0].message.content).toContain("SOURCES: 1");
    expect((await call(key, "PUT", "/_mock/strategy", { strategy: { kind: "magic" } })).status).toBe(400);
  });

  test("受け取ったリクエストを、効いた障害と状態コードと一緒に記録し、消せる", async () => {
    const key = newKey();
    await setFaults(key, [{ kind: "rateLimit", count: 1 }]);
    await embeddings(key, { model: "mock-embed-8", input: "記録される本文" });
    await embeddings(key, { model: "nope", input: "x" });
    const list = await requests(key);
    expect(list.map((r) => [r.status, r.faults])).toEqual([
      [429, ["rateLimit"]],
      [404, []],
    ]);
    expect(list[0]!.body).toEqual({ model: "mock-embed-8", input: "記録される本文" });
    expect(await requests(newKey())).toEqual([]);
    expect((await call(key, "DELETE", "/_mock/requests")).status).toBe(204);
    expect(await requests(key)).toEqual([]);
  });

  test(`記録はキーごとに直近の ${MAX_RECORDED} 件まで`, async () => {
    const key = newKey();
    for (let i = 0; i < MAX_RECORDED + 5; i++) await embeddings(key, { model: "mock-embed-1", input: `n${i}` });
    const list = await requests(key);
    expect(list).toHaveLength(MAX_RECORDED);
    expect(list[0]!.body).toEqual({ model: "mock-embed-1", input: "n5" });
  });
});
