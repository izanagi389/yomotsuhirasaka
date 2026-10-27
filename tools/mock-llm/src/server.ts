// 擬似LLMのHTTPサーバー（OpenAI互換の POST /v1/embeddings と POST /v1/chat/completions）。
// テストからの操作は /_mock/ の下（障害の設定・回答の組み立て方・受け取ったリクエストの記録）。
// どちらも Authorization: Bearer <キー> が要り、状態はキーごとに分かれる。

import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { composeAnswer, messageText, type ChatMessage } from "./chat.js";
import { embed, embeddingDimensions } from "./embed.js";
import { MockState, parseFaults, parseStrategy, type Endpoint, type Fault, type FaultKind } from "./state.js";
import { countTokens } from "./text.js";

export const CHAT_MODEL = "mock-chat";
const MAX_BODY_BYTES = 10 * 1024 * 1024;
const DEFAULT_TIMEOUT_DELAY_MS = 120_000;
/** ストリーミングで1つのチャンクに入れる文字数。 */
const STREAM_CHUNK_CHARS = 8;

export interface MockLlmOptions {
  port?: number;
  host?: string;
}

export interface MockLlm {
  /** 待ち受けているアドレス（`http://<host>:<port>`。末尾の / なし）。 */
  readonly url: string;
  readonly port: number;
  close(): Promise<void>;
}

class HttpError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

function sendJson(res: ServerResponse, status: number, body: unknown, headers: Record<string, string> = {}): void {
  const text = JSON.stringify(body);
  res.writeHead(status, { "content-type": "application/json", "content-length": Buffer.byteLength(text), ...headers });
  res.end(text);
}

function sendError(res: ServerResponse, error: HttpError, headers: Record<string, string> = {}): void {
  sendJson(res, error.status, { error: { message: error.message, type: "invalid_request_error", code: error.code } }, headers);
}

function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    let size = 0;
    req.on("data", (c: Buffer) => {
      size += c.length;
      if (size > MAX_BODY_BYTES) {
        reject(new HttpError(413, "request_too_large", "request body is too large"));
        req.destroy();
        return;
      }
      chunks.push(c);
    });
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    req.on("error", reject);
  });
}

function parseJson(text: string): unknown {
  try {
    return JSON.parse(text) as unknown;
  } catch {
    throw new HttpError(400, "invalid_json", "request body is not valid JSON");
  }
}

function apiKey(req: IncomingMessage): string {
  const m = /^Bearer\s+(.+)$/iu.exec(req.headers["authorization"] ?? "");
  const key = m?.[1]?.trim();
  if (!key) throw new HttpError(401, "invalid_api_key", "missing API key");
  return key;
}

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

function sleep(ms: number, res: ServerResponse): Promise<void> {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, ms);
    res.on("close", () => {
      clearTimeout(timer);
      resolve();
    });
  });
}

function embeddingsResponse(body: unknown, noUsage: boolean): unknown {
  if (!isObject(body) || typeof body["model"] !== "string") throw new HttpError(400, "invalid_request", "model is required");
  const dims = embeddingDimensions(body["model"]);
  if (dims === null) throw new HttpError(404, "model_not_found", `unknown model: ${body["model"]}`);
  const input = body["input"];
  const inputs = typeof input === "string" ? [input] : input;
  if (!Array.isArray(inputs) || inputs.length === 0 || !inputs.every((s) => typeof s === "string" && s.length > 0)) {
    throw new HttpError(400, "invalid_request", "input must be a non-empty string or an array of non-empty strings");
  }
  const tokens = (inputs as string[]).reduce((sum, s) => sum + countTokens(s), 0);
  return {
    object: "list",
    data: (inputs as string[]).map((s, index) => ({ object: "embedding", index, embedding: embed(s, dims) })),
    model: body["model"],
    ...(noUsage ? {} : { usage: { prompt_tokens: tokens, total_tokens: tokens } }),
  };
}

function chatMessages(body: unknown): ChatMessage[] {
  if (!isObject(body) || typeof body["model"] !== "string") throw new HttpError(400, "invalid_request", "model is required");
  if (body["model"] !== CHAT_MODEL) throw new HttpError(404, "model_not_found", `unknown model: ${body["model"]}`);
  const messages = body["messages"];
  const valid =
    Array.isArray(messages) &&
    messages.length > 0 &&
    messages.every(
      (m) =>
        isObject(m) &&
        typeof m["role"] === "string" &&
        (typeof m["content"] === "string" || (Array.isArray(m["content"]) && m["content"].every((p) => isObject(p) && typeof p["type"] === "string"))),
    );
  if (!valid) throw new HttpError(400, "invalid_request", "messages must be a non-empty array of {role, content}");
  return messages as ChatMessage[];
}

export async function startMockLlm(options: MockLlmOptions = {}): Promise<MockLlm> {
  const state = new MockState();
  let sequence = 0;

  async function handleApi(endpoint: Endpoint, key: string, body: unknown, res: ServerResponse): Promise<void> {
    const faults = state.takeFaults(key, endpoint);
    const record = { receivedAt: new Date().toISOString(), endpoint, body, faults: faults.map((f) => f.kind), status: 0 };
    state.record(key, record);
    try {
      await respond(endpoint, key, body, faults, record, res);
    } catch (e) {
      if (e instanceof HttpError) record.status = e.status;
      throw e;
    }
  }

  async function respond(
    endpoint: Endpoint,
    key: string,
    body: unknown,
    faults: Fault[],
    record: { status: number },
    res: ServerResponse,
  ): Promise<void> {
    const has = (kind: FaultKind): Fault | undefined => faults.find((f) => f.kind === kind);
    const timeout = has("timeout");
    if (timeout) await sleep(timeout.delayMs ?? DEFAULT_TIMEOUT_DELAY_MS, res);
    if (res.destroyed) return;
    const rateLimit = has("rateLimit");
    if (rateLimit) {
      record.status = 429;
      sendError(res, new HttpError(429, "rate_limit_exceeded", "rate limit exceeded"), { "retry-after": String(rateLimit.retryAfterSeconds ?? 1) });
      return;
    }
    if (has("serverError")) {
      record.status = 500;
      sendError(res, new HttpError(500, "server_error", "internal error"));
      return;
    }
    const stream = endpoint === "chat" && isObject(body) && body["stream"] === true;
    if (has("brokenJson")) {
      record.status = 200;
      if (stream) {
        res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
        res.end('data: {"id":"chatcmpl-mock",\n\ndata: [DONE]\n\n');
      } else {
        res.writeHead(200, { "content-type": "application/json" });
        res.end('{"object":"list","data":[');
      }
      return;
    }
    const noUsage = has("noUsage") !== undefined;

    if (endpoint === "embeddings") {
      const out = embeddingsResponse(body, noUsage);
      record.status = 200;
      sendJson(res, 200, out);
      return;
    }

    const messages = chatMessages(body);
    const answer = composeAnswer(messages, state.strategy(key), {
      noCitations: has("noCitations") !== undefined,
      unknownSource: has("unknownSource") !== undefined,
      undeclaredCitation: has("undeclaredCitation") !== undefined,
    });
    const promptTokens = messages.reduce((sum, m) => sum + countTokens(messageText(m)), 0);
    const completionTokens = countTokens(answer);
    const usage = { prompt_tokens: promptTokens, completion_tokens: completionTokens, total_tokens: promptTokens + completionTokens };
    const id = `chatcmpl-mock-${++sequence}`;
    const created = Math.floor(Date.now() / 1000);
    const cut = has("cutStream") !== undefined;
    record.status = 200;

    if (!stream) {
      const text = JSON.stringify({
        id,
        object: "chat.completion",
        created,
        model: CHAT_MODEL,
        choices: [{ index: 0, message: { role: "assistant", content: answer }, finish_reason: "stop" }],
        ...(noUsage ? {} : { usage }),
      });
      res.writeHead(200, { "content-type": "application/json" });
      if (cut) {
        res.write(text.slice(0, Math.ceil(text.length / 2)), () => res.destroy());
        return;
      }
      res.end(text);
      return;
    }

    const chunk = (delta: unknown, finishReason: string | null, extra: Record<string, unknown> = {}) =>
      `data: ${JSON.stringify({ id, object: "chat.completion.chunk", created, model: CHAT_MODEL, choices: [{ index: 0, delta, finish_reason: finishReason }], ...extra })}\n\n`;
    const chars = [...answer];
    const pieces: string[] = [];
    for (let i = 0; i < chars.length; i += STREAM_CHUNK_CHARS) pieces.push(chars.slice(i, i + STREAM_CHUNK_CHARS).join(""));
    res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
    res.write(chunk({ role: "assistant", content: "" }, null));
    const sent = cut ? pieces.slice(0, Math.ceil(pieces.length / 2)) : pieces;
    for (const piece of sent) res.write(chunk({ content: piece }, null));
    if (cut) {
      res.write("", () => res.destroy());
      return;
    }
    res.write(chunk({}, "stop"));
    const streamOptions = isObject(body) ? body["stream_options"] : undefined;
    const includeUsage = isObject(streamOptions) && streamOptions["include_usage"] === true;
    if (includeUsage && !noUsage) {
      res.write(`data: ${JSON.stringify({ id, object: "chat.completion.chunk", created, model: CHAT_MODEL, choices: [], usage })}\n\n`);
    }
    res.end("data: [DONE]\n\n");
  }

  function handleControl(path: string, method: string, key: string, body: unknown, res: ServerResponse): void {
    if (path === "/_mock/faults" && method === "PUT") {
      const faults = parseFaults(body);
      if (typeof faults === "string") throw new HttpError(400, "invalid_request", faults);
      state.setFaults(key, faults);
    } else if (path === "/_mock/faults" && method === "DELETE") {
      state.clearFaults(key);
    } else if (path === "/_mock/strategy" && method === "PUT") {
      const strategy = parseStrategy(body);
      if (typeof strategy === "string") throw new HttpError(400, "invalid_request", strategy);
      state.setStrategy(key, strategy);
    } else if (path === "/_mock/strategy" && method === "DELETE") {
      state.resetStrategy(key);
    } else if (path === "/_mock/requests" && method === "GET") {
      sendJson(res, 200, { requests: state.requests(key) });
      return;
    } else if (path === "/_mock/requests" && method === "DELETE") {
      state.clearRequests(key);
    } else {
      throw new HttpError(404, "not_found", "not found");
    }
    res.writeHead(204).end();
  }

  const server = createServer((req, res) => {
    void (async () => {
      const path = (req.url ?? "").split("?")[0] ?? "";
      const method = req.method ?? "";
      try {
        const key = apiKey(req);
        const text = await readBody(req);
        const body = text === "" ? undefined : parseJson(text);
        if (path.startsWith("/_mock/")) {
          handleControl(path, method, key, body, res);
        } else if (method === "POST" && path === "/v1/embeddings") {
          await handleApi("embeddings", key, body, res);
        } else if (method === "POST" && path === "/v1/chat/completions") {
          await handleApi("chat", key, body, res);
        } else {
          throw new HttpError(404, "not_found", "not found");
        }
      } catch (e) {
        if (res.headersSent || res.destroyed) return;
        if (e instanceof HttpError) sendError(res, e);
        else sendError(res, new HttpError(500, "server_error", "internal error"));
      }
    })();
  });

  const host = options.host ?? "127.0.0.1";
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(options.port ?? 0, host, () => resolve());
  });
  const port = (server.address() as AddressInfo).port;
  return {
    url: `http://${host.includes(":") ? `[${host}]` : host}:${port}`,
    port,
    close: () =>
      new Promise<void>((resolve) => {
        server.closeAllConnections();
        server.close(() => resolve());
      }),
  };
}
