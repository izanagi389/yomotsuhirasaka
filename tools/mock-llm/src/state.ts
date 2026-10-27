// API キーごとの状態（障害の設定・回答の組み立て方・受け取ったリクエストの記録）。
// テストを同時に流しても混ざらないよう、キーごとに分ける。記録はメモリの中だけに持つ。

import { DEFAULT_STRATEGY, type Strategy } from "./chat.js";

export type Endpoint = "embeddings" | "chat";

export const FAULT_KINDS = [
  "timeout",
  "rateLimit",
  "serverError",
  "brokenJson",
  "noUsage",
  "noCitations",
  "unknownSource",
  "undeclaredCitation",
  "cutStream",
] as const;
export type FaultKind = (typeof FAULT_KINDS)[number];

/** チャットにだけ効く障害。 */
const CHAT_ONLY: ReadonlySet<FaultKind> = new Set(["noCitations", "unknownSource", "undeclaredCitation", "cutStream"]);

export interface Fault {
  kind: FaultKind;
  /** 効く API。省略すると両方（チャットにだけ効く障害は、チャットだけ）。 */
  endpoint?: Endpoint;
  /** 次の n 回だけ効く。省略すると、消すまで続く。 */
  count?: number;
  /** timeout: 応答を始めるまで待つ時間（既定 120000）。 */
  delayMs?: number;
  /** rateLimit: Retry-After の秒数（既定 1）。 */
  retryAfterSeconds?: number;
}

export interface RecordedRequest {
  receivedAt: string;
  endpoint: Endpoint;
  /** 受け取った本文（JSON）。 */
  body: unknown;
  /** 効いた障害。 */
  faults: FaultKind[];
  status: number;
}

export const MAX_RECORDED = 1000;

interface KeyState {
  faults: { fault: Fault; remaining: number | null }[];
  strategy: Strategy;
  requests: RecordedRequest[];
}

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const isCount = (v: unknown): v is number => typeof v === "number" && Number.isInteger(v) && v >= 0;

/** 障害の設定の検証。不正なら理由を返す。 */
export function parseFaults(body: unknown): Fault[] | string {
  if (!isObject(body) || !Array.isArray(body["faults"])) return "body must be {\"faults\": [...]}";
  const out: Fault[] = [];
  for (const f of body["faults"] as unknown[]) {
    if (!isObject(f) || !FAULT_KINDS.includes(f["kind"] as FaultKind)) return `unknown fault kind: ${JSON.stringify(isObject(f) ? f["kind"] : f)}`;
    const kind = f["kind"] as FaultKind;
    const endpoint = f["endpoint"];
    if (endpoint !== undefined && endpoint !== "embeddings" && endpoint !== "chat") return "endpoint must be \"embeddings\" or \"chat\"";
    if (endpoint === "embeddings" && CHAT_ONLY.has(kind)) return `${kind} applies to chat only`;
    for (const name of ["count", "delayMs", "retryAfterSeconds"] as const) {
      if (f[name] !== undefined && !isCount(f[name])) return `${name} must be a non-negative integer`;
    }
    out.push({
      kind,
      ...(endpoint !== undefined ? { endpoint } : {}),
      ...(f["count"] !== undefined ? { count: f["count"] as number } : {}),
      ...(f["delayMs"] !== undefined ? { delayMs: f["delayMs"] as number } : {}),
      ...(f["retryAfterSeconds"] !== undefined ? { retryAfterSeconds: f["retryAfterSeconds"] as number } : {}),
    });
  }
  return out;
}

/** 回答の組み立て方の検証。不正なら理由を返す。 */
export function parseStrategy(body: unknown): Strategy | string {
  const s = isObject(body) ? body["strategy"] : undefined;
  if (!isObject(s)) return "body must be {\"strategy\": {...}}";
  if (s["kind"] === "fixed") return typeof s["content"] === "string" ? { kind: "fixed", content: s["content"] } : "fixed needs content";
  if (s["kind"] === "firstN") {
    if (s["n"] !== undefined && !isCount(s["n"])) return "n must be a non-negative integer";
    return { kind: "firstN", ...(s["n"] !== undefined ? { n: s["n"] as number } : {}) };
  }
  if (s["kind"] === "overlap") {
    for (const name of ["maxSources", "minShared"] as const) {
      if (s[name] !== undefined && !isCount(s[name])) return `${name} must be a non-negative integer`;
    }
    return {
      kind: "overlap",
      ...(s["maxSources"] !== undefined ? { maxSources: s["maxSources"] as number } : {}),
      ...(s["minShared"] !== undefined ? { minShared: s["minShared"] as number } : {}),
    };
  }
  return "strategy kind must be overlap, firstN or fixed";
}

export class MockState {
  private readonly keys = new Map<string, KeyState>();

  private state(key: string): KeyState {
    let s = this.keys.get(key);
    if (!s) {
      s = { faults: [], strategy: DEFAULT_STRATEGY, requests: [] };
      this.keys.set(key, s);
    }
    return s;
  }

  setFaults(key: string, faults: Fault[]): void {
    this.state(key).faults = faults.map((fault) => ({ fault, remaining: fault.count ?? null }));
  }

  clearFaults(key: string): void {
    this.state(key).faults = [];
  }

  setStrategy(key: string, strategy: Strategy): void {
    this.state(key).strategy = strategy;
  }

  resetStrategy(key: string): void {
    this.state(key).strategy = DEFAULT_STRATEGY;
  }

  strategy(key: string): Strategy {
    return this.state(key).strategy;
  }

  /** この呼び出しに効く障害を取り出し、回数を数える（回数を使い切った障害は消す）。 */
  takeFaults(key: string, endpoint: Endpoint): Fault[] {
    const s = this.state(key);
    const applied: Fault[] = [];
    for (const entry of s.faults) {
      const { fault } = entry;
      if (fault.endpoint !== undefined && fault.endpoint !== endpoint) continue;
      if (endpoint === "embeddings" && CHAT_ONLY.has(fault.kind)) continue;
      if (entry.remaining === 0) continue;
      if (entry.remaining !== null) entry.remaining--;
      applied.push(fault);
    }
    s.faults = s.faults.filter((e) => e.remaining !== 0);
    return applied;
  }

  record(key: string, request: RecordedRequest): void {
    const list = this.state(key).requests;
    list.push(request);
    if (list.length > MAX_RECORDED) list.splice(0, list.length - MAX_RECORDED);
  }

  requests(key: string): RecordedRequest[] {
    return [...this.state(key).requests];
  }

  clearRequests(key: string): void {
    this.state(key).requests = [];
  }
}
