// テストの実行環境で待ち受ける受信サーバー（Webhookの受け手、擬似HIBPなど）。
// すべてのアドレスで待ち受け、どのアドレスに届いたか（localAddress）も記録する。

import { createServer as createHttpServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { createServer as createHttpsServer } from "node:https";
import { readFileSync } from "node:fs";
import type { AddressInfo } from "node:net";

export interface ReceivedRequest {
  method: string;
  /** パスとクエリ。 */
  path: string;
  headers: Record<string, string>;
  body: string;
  /** 受け付けたこちら側のアドレス（どのIPアドレス宛てに届いたか）。 */
  localAddress: string;
  receivedAt: number;
}

export type Handler = (req: ReceivedRequest, res: ServerResponse) => void;

export interface Receiver {
  readonly port: number;
  readonly scheme: "http" | "https";
  readonly requests: ReceivedRequest[];
  /** 指定したホスト名（またはIPアドレス）とパスのURL。 */
  url(host: string, path?: string): string;
  /** パスごとの応答を決める（既定は 200 で本文 `ok`）。 */
  on(path: string, handler: Handler): void;
  /** どのパスにも当てはまらないときの応答を決める。 */
  fallback(handler: Handler): void;
  /** 条件に合うリクエストが届くまで待つ。 */
  waitFor(predicate: (r: ReceivedRequest) => boolean, timeoutMs?: number): Promise<ReceivedRequest>;
  /** 指定した時間だけ待ち、その間に届いた条件に合うリクエストを返す。 */
  collect(predicate: (r: ReceivedRequest) => boolean, waitMs: number): Promise<ReceivedRequest[]>;
  close(): Promise<void>;
}

function normalizeHeaders(req: IncomingMessage): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(req.headers)) {
    if (v !== undefined) out[k.toLowerCase()] = Array.isArray(v) ? v.join(", ") : v;
  }
  return out;
}

export async function startReceiver(options: { tls?: { certFile: string; keyFile: string } } = {}): Promise<Receiver> {
  const requests: ReceivedRequest[] = [];
  const handlers = new Map<string, Handler>();
  let fallbackHandler: Handler | null = null;
  const waiters = new Set<() => void>();

  const listener = (req: IncomingMessage, res: ServerResponse) => {
    const chunks: Buffer[] = [];
    req.on("data", (c: Buffer) => chunks.push(c));
    req.on("end", () => {
      const received: ReceivedRequest = {
        method: req.method ?? "",
        path: req.url ?? "",
        headers: normalizeHeaders(req),
        body: Buffer.concat(chunks).toString("utf8"),
        localAddress: (req.socket.localAddress ?? "").replace(/^::ffff:/, ""),
        receivedAt: Date.now(),
      };
      requests.push(received);
      const pathOnly = received.path.split("?")[0] ?? "";
      const handler = handlers.get(pathOnly) ?? fallbackHandler;
      if (handler) handler(received, res);
      else {
        res.writeHead(200, { "content-type": "text/plain" });
        res.end("ok");
      }
      for (const w of waiters) w();
    });
  };

  const server: Server = options.tls
    ? createHttpsServer({ cert: readFileSync(options.tls.certFile), key: readFileSync(options.tls.keyFile) }, listener)
    : createHttpServer(listener);
  await new Promise<void>((resolve) => server.listen(0, "0.0.0.0", resolve));
  const port = (server.address() as AddressInfo).port;
  const scheme = options.tls ? "https" : "http";

  return {
    port,
    scheme,
    requests,
    url: (host, path = "/") => `${scheme}://${host}:${port}${path}`,
    on: (path, handler) => {
      handlers.set(path, handler);
    },
    fallback: (handler) => {
      fallbackHandler = handler;
    },
    waitFor(predicate, timeoutMs = 15_000) {
      return new Promise((resolve, reject) => {
        const check = () => {
          const found = requests.find(predicate);
          if (found) {
            cleanup();
            resolve(found);
          }
        };
        const timer = setTimeout(() => {
          cleanup();
          reject(new Error(`受信サーバーに、条件に合うリクエストが ${timeoutMs}ms 以内に届かなかった`));
        }, timeoutMs);
        const cleanup = () => {
          clearTimeout(timer);
          waiters.delete(check);
        };
        waiters.add(check);
        check();
      });
    },
    async collect(predicate, waitMs) {
      await new Promise((r) => setTimeout(r, waitMs));
      return requests.filter(predicate);
    },
    close: () =>
      new Promise((resolve) => {
        server.closeAllConnections?.();
        server.close(() => resolve());
      }),
  };
}
