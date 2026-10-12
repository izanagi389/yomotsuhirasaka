// テストの実行環境で動かす、`inv.test` のゾーンだけに答える小さなDNSサーバー（UDP、Aレコードのみ）。
// 名前ごとに「答えるIPアドレスの順番」を決められる。DNSリバインディングの検査（INV-WH-01）に使う。

import { createSocket, type Socket } from "node:dgram";

export interface DnsServer {
  readonly port: number;
  /**
   * 名前に答えるIPアドレスを決める。問い合わせのたびに配列を順に使い、最後の値を使い続ける。
   * TTLは0で答える。
   */
  set(name: string, answers: string[]): void;
  /** 名前ごとの問い合わせの回数。 */
  queryCount(name: string): number;
  close(): Promise<void>;
}

function readName(msg: Buffer, offset: number): { name: string; next: number } {
  const labels: string[] = [];
  let i = offset;
  for (;;) {
    const len = msg[i];
    if (len === undefined) throw new Error("DNS: 名前が途中で切れている");
    if (len === 0) return { name: labels.join(".").toLowerCase(), next: i + 1 };
    if ((len & 0xc0) !== 0) throw new Error("DNS: 問い合わせの圧縮には対応しない");
    labels.push(msg.subarray(i + 1, i + 1 + len).toString("ascii"));
    i += 1 + len;
  }
}

export function buildResponse(query: Buffer, answer: string | null): Buffer {
  const id = query.readUInt16BE(0);
  const flags = query.readUInt16BE(2);
  const { next } = readName(query, 12);
  const question = query.subarray(12, next + 4); // 名前 + QTYPE + QCLASS
  const qtype = query.readUInt16BE(next);
  const rcodeNxDomain = answer === null;
  const header = Buffer.alloc(12);
  header.writeUInt16BE(id, 0);
  // QR=1, OPCODE はそのまま, AA=1, RD はそのまま, RA=0, RCODE
  header.writeUInt16BE(0x8000 | (flags & 0x7900) | 0x0400 | (rcodeNxDomain ? 3 : 0), 2);
  header.writeUInt16BE(1, 4);
  const giveAnswer = answer !== null && qtype === 1;
  header.writeUInt16BE(giveAnswer ? 1 : 0, 6);
  header.writeUInt16BE(0, 8);
  header.writeUInt16BE(0, 10);
  if (!giveAnswer) return Buffer.concat([header, question]);
  const rr = Buffer.alloc(16);
  rr.writeUInt16BE(0xc00c, 0); // 質問の名前を指す
  rr.writeUInt16BE(1, 2); // A
  rr.writeUInt16BE(1, 4); // IN
  rr.writeUInt32BE(0, 6); // TTL 0
  rr.writeUInt16BE(4, 10);
  answer!.split(".").forEach((o, i) => rr.writeUInt8(Number(o), 12 + i));
  return Buffer.concat([header, question, rr]);
}

export async function startDnsServer(port: number, host = "0.0.0.0"): Promise<DnsServer> {
  const table = new Map<string, string[]>();
  const counts = new Map<string, number>();
  const socket: Socket = createSocket("udp4");

  socket.on("message", (msg, rinfo) => {
    try {
      const { name } = readName(msg, 12);
      const n = (counts.get(name) ?? 0) + 1;
      counts.set(name, n);
      const answers = table.get(name);
      const answer = answers && answers.length > 0 ? answers[Math.min(n - 1, answers.length - 1)]! : null;
      socket.send(buildResponse(msg, answer), rinfo.port, rinfo.address);
    } catch {
      // 壊れた問い合わせには答えない
    }
  });

  await new Promise<void>((resolve, reject) => {
    socket.once("error", reject);
    socket.bind(port, host, () => resolve());
  });

  return {
    port: socket.address().port,
    set: (name, answers) => {
      table.set(name.toLowerCase(), answers);
      counts.set(name.toLowerCase(), 0);
    },
    queryCount: (name) => counts.get(name.toLowerCase()) ?? 0,
    close: () => new Promise((resolve) => socket.close(() => resolve())),
  };
}
