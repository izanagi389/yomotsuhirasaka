// テスト環境のメールの受け口から、メールを待って取り出す。

import { driver, type MailMessage } from "./driver.js";

/** 今ある分のメールのIDを覚えておき、それより後に届いたものだけを待てるようにする。 */
export async function mailCursor(to: string): Promise<Set<string>> {
  return new Set((await driver.mail.list(to)).map((m) => m.id));
}

/** seen にない新しいメールが届くまで待つ。 */
export async function waitForNewMail(to: string, seen: Set<string>, timeoutMs = 20_000): Promise<MailMessage> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const fresh = (await driver.mail.list(to)).filter((m) => !seen.has(m.id));
    if (fresh.length > 0) return fresh[fresh.length - 1]!;
    if (Date.now() > deadline) throw new Error(`${to} 宛てのメールが ${timeoutMs}ms 以内に届かなかった`);
    await new Promise((r) => setTimeout(r, 300));
  }
}

/** 指定した時間だけ待ち、seen にない新しいメールを返す（届かないことを確かめる用）。 */
export async function newMailWithin(to: string, seen: Set<string>, waitMs: number): Promise<MailMessage[]> {
  await new Promise((r) => setTimeout(r, waitMs));
  return (await driver.mail.list(to)).filter((m) => !seen.has(m.id));
}

/** 新しく届いたリセット・招待のメールからトークンを取り出す。 */
export async function waitForToken(to: string, seen: Set<string>, kind: "reset" | "invite"): Promise<string> {
  const message = await waitForNewMail(to, seen);
  const token = driver.mail.tokenFrom(message, kind);
  if (!token) throw new Error(`${to} 宛てのメールに${kind === "reset" ? "リセット" : "招待"}のトークンがない`);
  return token;
}
