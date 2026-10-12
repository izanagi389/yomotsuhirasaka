// 擬似の漏えいパスワードの照会先（Have I Been Pwned の range API と同じ形: GET /range/<SHA-1の先頭5文字>）。

import { createHash } from "node:crypto";
import { startReceiver, type Receiver } from "./net.js";

export const sha1Hex = (s: string): string => createHash("sha1").update(s, "utf8").digest("hex").toUpperCase();

export interface MockBreachApi {
  readonly receiver: Receiver;
  /** CMSの設定に渡す照会先のURL（末尾の / なし）。 */
  baseUrl(host: string): string;
  /** 漏えいしたものとして答えるパスワードを足す。 */
  addBreached(password: string): void;
  /** 照会先が落ちている状態にする（すべて 503 で答える）。 */
  setDown(down: boolean): void;
  close(): Promise<void>;
}

export async function startMockBreachApi(): Promise<MockBreachApi> {
  const receiver = await startReceiver();
  const breached = new Set<string>();
  let down = false;

  receiver.fallback((req, res) => {
    const m = /^\/range\/([0-9A-Fa-f]{5})$/.exec(req.path.split("?")[0] ?? "");
    if (down || !m) {
      res.writeHead(down ? 503 : 404).end();
      return;
    }
    const prefix = m[1]!.toUpperCase();
    const lines = [...breached]
      .map(sha1Hex)
      .filter((h) => h.startsWith(prefix))
      .map((h) => `${h.slice(5)}:1000`);
    // 本物と同じく、関係のない行も混ぜて返す
    lines.push(`${"0".repeat(35)}:1`);
    res.writeHead(200, { "content-type": "text/plain" });
    res.end(lines.join("\r\n"));
  });

  return {
    receiver,
    baseUrl: (host) => receiver.url(host, "").replace(/\/$/, ""),
    addBreached: (password) => {
      breached.add(password);
    },
    setDown: (value) => {
      down = value;
    },
    close: () => receiver.close(),
  };
}
