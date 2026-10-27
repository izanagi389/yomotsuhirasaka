// 擬似LLM（tools/mock-llm）。ドライバーの一部。
//
// - テストの舞台を作るとき（global-setup.ts）に、ビルド済みの dist/main.js を別のプロセスとして起動する。
//   CMS_TEST_MOCK_LLM_BIN で別の場所を指定できる。CMSからは普通の接続先（OpenAI互換の /v1）に見える。
// - 障害の設定・回答の組み立て方・受け取ったリクエストの記録は、APIキーごとに分かれる。
//   テストは自分のワークスペース用のキーを使い、他のテストと混ざらないようにする。

import { spawn, type ChildProcess } from "node:child_process";
import { existsSync } from "node:fs";
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

/** 擬似LLMの操作。どれも、指定したAPIキーの状態だけを変える。 */
export const mockLlm = {
  /** CMSに設定する接続先。 */
  get baseUrl(): string {
    return env.mockLlmUrl;
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
