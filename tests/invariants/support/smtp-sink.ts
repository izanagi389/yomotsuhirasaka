// テスト環境のメールの受け口（最小限のSMTPの受け手）。README の「Mailpitなど」の代わり。
//
// - 受け取ったメールを、1通ずつJSONのファイル（宛先と生のメール）にしてディレクトリに置く。テストのプロセスはこれを読む。
// - 暗号化と認証は扱わない（CMSの設定は security = "none"。ループバックの宛先にだけ許される）。

import { randomUUID } from "node:crypto";
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { createServer, type Server } from "node:net";
import { join } from "node:path";
import type { MailMessage } from "./driver.js";

export interface SmtpSink {
  port: number;
  close(): Promise<void>;
}

/** SMTPの受け手を起動する。メールは dir に置く。 */
export function startSmtpSink(dir: string): Promise<SmtpSink> {
  const server: Server = createServer((socket) => {
    let buffer = "";
    let inData = false;
    let data: string[] = [];
    let rcpt: string[] = [];
    const reply = (line: string) => socket.write(`${line}\r\n`);
    reply("220 inv.test ESMTP test sink");
    socket.setEncoding("latin1");
    socket.on("data", (chunk: string) => {
      buffer += chunk;
      let at: number;
      while ((at = buffer.indexOf("\r\n")) >= 0) {
        const line = buffer.slice(0, at);
        buffer = buffer.slice(at + 2);
        if (inData) {
          if (line === ".") {
            inData = false;
            const id = `${Date.now()}-${randomUUID()}`;
            writeFileSync(join(dir, `${id}.json`), JSON.stringify({ id, to: rcpt, raw: data.join("\r\n") }));
            data = [];
            rcpt = [];
            reply("250 OK");
          } else {
            data.push(line.startsWith("..") ? line.slice(1) : line);
          }
          continue;
        }
        const verb = line.slice(0, 4).toUpperCase();
        if (verb === "EHLO") {
          reply("250-inv.test");
          reply("250 8BITMIME");
        } else if (verb === "RCPT") {
          const m = line.match(/<([^>]*)>/);
          if (m) rcpt.push(m[1]!.toLowerCase());
          reply("250 OK");
        } else if (verb === "DATA") {
          inData = true;
          reply("354 End data with <CR><LF>.<CR><LF>");
        } else if (verb === "RSET") {
          rcpt = [];
          data = [];
          reply("250 OK");
        } else if (verb === "QUIT") {
          reply("221 Bye");
          socket.end();
        } else {
          reply("250 OK");
        }
      }
    });
    socket.on("error", () => socket.destroy());
  });
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const port = (server.address() as { port: number }).port;
      resolve({ port, close: () => new Promise((r) => server.close(() => r())) });
    });
  });
}

/** 文字列（latin1で受け取ったバイト列）を UTF-8 として読み直す。 */
const utf8 = (latin1: string): string => Buffer.from(latin1, "latin1").toString("utf8");

function decodeQuotedPrintable(s: string): string {
  const bytes = s.replace(/=\r?\n/g, "").replace(/=([0-9A-Fa-f]{2})/g, (_, h: string) => String.fromCharCode(parseInt(h, 16)));
  return utf8(bytes);
}

/** 件名などの符号化された語（RFC 2047）を読む。 */
function decodeWords(s: string): string {
  return s.replace(/=\?([^?]+)\?([BbQq])\?([^?]*)\?=\s*/g, (_, _charset: string, enc: string, text: string) =>
    enc.toUpperCase() === "B" ? Buffer.from(text, "base64").toString("utf8") : decodeQuotedPrintable(text.replace(/_/g, " ")),
  );
}

/** 受け取ったメールを読む。 */
export function parseMail(id: string, to: string[], raw: string): MailMessage {
  const split = raw.indexOf("\r\n\r\n");
  const head = split >= 0 ? raw.slice(0, split) : raw;
  const body = split >= 0 ? raw.slice(split + 4) : "";
  const headers = new Map<string, string>();
  for (const line of head.replace(/\r\n[ \t]+/g, " ").split("\r\n")) {
    const at = line.indexOf(":");
    if (at > 0) headers.set(line.slice(0, at).trim().toLowerCase(), line.slice(at + 1).trim());
  }
  const encoding = (headers.get("content-transfer-encoding") ?? "7bit").toLowerCase();
  const text =
    encoding === "base64"
      ? Buffer.from(body.replace(/\s+/g, ""), "base64").toString("utf8")
      : encoding === "quoted-printable"
        ? decodeQuotedPrintable(body)
        : utf8(body);
  return { id, to, subject: decodeWords(headers.get("subject") ?? ""), text, raw: utf8(raw) };
}

/** dir に置いたメールのうち、to 宛てのもの（届いた順）。 */
export function listMail(dir: string, to: string): MailMessage[] {
  const want = to.toLowerCase();
  return readdirSync(dir)
    .filter((f) => f.endsWith(".json"))
    .sort()
    .map((f) => JSON.parse(readFileSync(join(dir, f), "utf8")) as { id: string; to: string[]; raw: string })
    .filter((m) => m.to.includes(want))
    .map((m) => parseMail(m.id, m.to, m.raw));
}
