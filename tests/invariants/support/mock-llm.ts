// 擬似LLM（tools/mock-llm）。ドライバーの一部。
//
// - テストの舞台を作るとき（global-setup.ts）に、ビルド済みの dist/main.js を別のプロセスとして起動する。
//   CMS_TEST_MOCK_LLM_BIN で別の場所を指定できる。CMSからは普通の接続先（OpenAI互換の /v1）に見える。
// - 障害の設定・回答の組み立て方・受け取ったリクエストの記録は、APIキーごとに分かれる。
//   テストは自分のワークスペース用のキーを使い、他のテストと混ざらないようにする。

import { spawn, type ChildProcess } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { env } from "./env.js";

export interface StartedMockLlm {
  /** CMSに設定するOpenAI互換のAPIの接続先（`http://127.0.0.1:<port>/v1`）。 */
  readonly baseUrl: string;
  stop(): Promise<void>;
}

/** 擬似LLMを別のプロセスとして起動し、待ち受けを始めるまで待つ。 */
export async function startMockLlmProcess(): Promise<StartedMockLlm> {
  const bin = process.env["CMS_TEST_MOCK_LLM_BIN"] ?? fileURLToPath(new URL("../../../tools/mock-llm/dist/main.js", import.meta.url));
  if (!existsSync(bin)) {
    throw new Error(`擬似LLMがビルドされていません: ${bin}（npm run build -w @new-cms/mock-llm）`);
  }
  const child: ChildProcess = spawn(process.execPath, [bin], {
    env: { ...process.env, PORT: "0", HOST: "127.0.0.1" },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "";
  const url = await new Promise<string>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`擬似LLMが起動しない:\n${output}`)), 10_000);
    const onData = (chunk: Buffer) => {
      output += chunk.toString("utf8");
      const m = /mock-llm listening on (\S+)/u.exec(output);
      if (m) {
        clearTimeout(timer);
        resolve(m[1]!);
      }
    };
    child.stdout!.on("data", onData);
    child.stderr!.on("data", onData);
    child.once("exit", (code) => {
      clearTimeout(timer);
      reject(new Error(`擬似LLMが終了した（${code}）:\n${output}`));
    });
  });
  return {
    baseUrl: `${url}/v1`,
    stop: () =>
      new Promise<void>((resolve) => {
        if (child.exitCode !== null) return resolve();
        child.once("exit", () => resolve());
        child.kill("SIGTERM");
      }),
  };
}

/** 擬似LLMに起こさせる障害（tools/mock-llm/src/state.ts の Fault と同じ形）。 */
export interface MockLlmFault {
  kind:
    | "timeout"
    | "rateLimit"
    | "serverError"
    | "brokenJson"
    | "noUsage"
    | "noCitations"
    | "unknownSource"
    | "undeclaredCitation"
    | "cutStream";
  endpoint?: "embeddings" | "chat";
  count?: number;
  delayMs?: number;
  retryAfterSeconds?: number;
}

/** チャットの回答の組み立て方（tools/mock-llm/src/chat.ts の Strategy と同じ形）。 */
export type MockLlmStrategy =
  | { kind: "overlap"; maxSources?: number; minShared?: number }
  | { kind: "firstN"; n?: number }
  | { kind: "fixed"; content: string };

export interface MockLlmRequest {
  receivedAt: string;
  endpoint: "embeddings" | "chat";
  body: unknown;
  faults: string[];
  status: number;
}

async function control(apiKey: string, method: string, path: string, body?: unknown): Promise<string> {
  const base = env.mockLlmUrl.replace(/\/v1$/u, "");
  const res = await fetch(`${base}${path}`, {
    method,
    headers: { authorization: `Bearer ${apiKey}`, ...(body !== undefined ? { "content-type": "application/json" } : {}) },
    ...(body !== undefined ? { body: JSON.stringify(body) } : {}),
  });
  const text = await res.text();
  if (!res.ok) throw new Error(`擬似LLMの操作 ${method} ${path} が失敗した（${res.status}）: ${text}`);
  return text;
}

/** 擬似LLMのトークン数の数え方（tools/mock-llm の usage と同じ: 文字数 ÷ 4 の切り上げ、少なくとも 1）。 */
export function mockUsageTokens(inputs: readonly string[]): number {
  return inputs.reduce((sum, s) => sum + Math.max(1, Math.ceil([...s].length / 4)), 0);
}

/**
 * 質問応答のプロンプトと回答の形（tools/mock-llm/src/format.json。擬似LLMとCMSが同じファイルを読む）。
 * テストは、回答の組み立て方 `fixed` に渡す回答を、この形で作る。
 */
const FORMAT = JSON.parse(readFileSync(fileURLToPath(new URL("../../../tools/mock-llm/src/format.json", import.meta.url)), "utf8")) as {
  answerableDeclaration: string;
  notAnswerableDeclaration: string;
  sourceSeparator: string;
  noConflict: string;
};

/** 回答の組み立て方 `fixed` に渡す回答（1行目が判定の先行宣言、2行目からが本文）。sources が空なら「答えられない」の宣言。 */
export function fixedAnswer(sources: readonly number[], body: string): string {
  const declaration =
    sources.length === 0 ? FORMAT.notAnswerableDeclaration : FORMAT.answerableDeclaration.replace("{sources}", sources.join(FORMAT.sourceSeparator));
  return `${declaration}\n${body}`;
}

/** 矛盾の検知（編集者向けのAI支援。M4）で、食い違いがないときの LLM の出力（format.json の noConflict）。 */
export const noConflictOutput = (): string => FORMAT.noConflict;

/** 本文の中の引用記号（`[n]` の形。format.json の citation と同じ）。 */
export const citation = (n: number): string => `[${n}]`;

/** 受け取ったチャットのリクエスト。 */
export function chatRequests(requests: readonly MockLlmRequest[]): MockLlmRequest[] {
  return requests.filter((r) => r.endpoint === "chat");
}

/** 文字数（Unicode のコードポイント）の合計。 */
export function characterCount(inputs: readonly string[]): number {
  return inputs.reduce((sum, s) => sum + [...s].length, 0);
}

/** 受け取った埋め込みのリクエストの入力（すべてのリクエストの分を、届いた順に）。 */
export function embeddingInputs(requests: readonly MockLlmRequest[]): string[] {
  return requests
    .filter((r) => r.endpoint === "embeddings")
    .flatMap((r) => {
      const input = (r.body as { input?: unknown } | undefined)?.input;
      return typeof input === "string" ? [input] : Array.isArray(input) ? input.filter((x): x is string => typeof x === "string") : [];
    });
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** 擬似LLMの操作。どれも、指定したAPIキーの状態だけを変える。 */
export const mockLlm = {
  /** CMSに設定する接続先。 */
  get baseUrl(): string {
    return env.mockLlmUrl;
  },
  /** このキーで擬似LLMにつなぐ、サーバーの設定の LLM の接続先（埋め込みのモデルは 64 次元）。 */
  connection(apiKey: string): { baseUrl: string; apiKey: string; embeddingModel: string; embeddingDimensions: number } {
    return { baseUrl: env.mockLlmUrl, apiKey, embeddingModel: "mock-embed-64", embeddingDimensions: 64 };
  },
  /** 擬似LLMの、チャットのモデルの名前（回答の生成。M4）。 */
  chatModel: "mock-chat",
  /** 条件に合うまで、受け取ったリクエストを読み直す。時間内に合わなければ失敗する。 */
  async waitForRequests(apiKey: string, predicate: (requests: MockLlmRequest[]) => boolean, timeoutMs = 60_000): Promise<MockLlmRequest[]> {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const list = await mockLlm.requests(apiKey);
      if (predicate(list)) return list;
      if (Date.now() > deadline) throw new Error(`擬似LLMに、待っていたリクエストが ${timeoutMs}ms 以内に届かない（届いた数: ${list.length}）`);
      await sleep(500);
    }
  },
  /** 受け取ったリクエストの数が quietMs の間増えなくなるまで待ち、その時点の一覧を返す。 */
  async waitUntilQuiet(apiKey: string, quietMs = 8_000, timeoutMs = 120_000): Promise<MockLlmRequest[]> {
    const deadline = Date.now() + timeoutMs;
    let list = await mockLlm.requests(apiKey);
    let since = Date.now();
    for (;;) {
      await sleep(500);
      const next = await mockLlm.requests(apiKey);
      if (next.length !== list.length) {
        list = next;
        since = Date.now();
      } else if (Date.now() - since >= quietMs) {
        return list;
      }
      if (Date.now() > deadline) throw new Error(`擬似LLMへのリクエストが ${timeoutMs}ms たっても止まらない（届いた数: ${list.length}）`);
    }
  },
  async setFaults(apiKey: string, faults: MockLlmFault[]): Promise<void> {
    await control(apiKey, "PUT", "/_mock/faults", { faults });
  },
  async clearFaults(apiKey: string): Promise<void> {
    await control(apiKey, "DELETE", "/_mock/faults");
  },
  async setStrategy(apiKey: string, strategy: MockLlmStrategy): Promise<void> {
    await control(apiKey, "PUT", "/_mock/strategy", { strategy });
  },
  async resetStrategy(apiKey: string): Promise<void> {
    await control(apiKey, "DELETE", "/_mock/strategy");
  },
  /** 受け取ったリクエスト（古い順。キーごとに直近の1000件）。 */
  async requests(apiKey: string): Promise<MockLlmRequest[]> {
    return (JSON.parse(await control(apiKey, "GET", "/_mock/requests")) as { requests: MockLlmRequest[] }).requests;
  },
  async clearRequests(apiKey: string): Promise<void> {
    await control(apiKey, "DELETE", "/_mock/requests");
  },
};
