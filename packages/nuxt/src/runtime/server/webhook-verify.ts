// CMSからのWebhookの署名の検証（13-security.md「外部への送信」の「Webhookの署名」、Q188）。
// ここは Nitro に依存しない（テストで直接呼ぶ）。
//
// - ヘッダー: `Webhook-Signature: t=<UNIX秒>,id=<イベントID>,v1=<16進>[,v1=<16進>]`（秘密の入れ替え中は v1 が複数並ぶ）
// - 署名の対象: `<t>.<id>.<本文>` を HMAC-SHA256 で署名したもの
// - 送信時刻が今から離れすぎているもの、一度受け付けたイベントIDは受け付けない（古いものや同じものの再送を拒否する）
// - 比べるときは、長さの違いも含めて時間が変わらない比べ方をする

import { createHmac, timingSafeEqual } from "node:crypto";

/** 送信時刻と今との差の上限（秒）。 */
export const MAX_SKEW_SECONDS = 300;
/** 本文の大きさの上限（バイト）。CMSのWebhookの本文はIDとURLだけなので小さい。 */
export const MAX_BODY_BYTES = 64 * 1024;

export interface ParsedSignature {
  timestamp: number;
  eventId: string;
  signatures: string[];
}

/** ヘッダーを読む。形が違えば null。 */
export function parseSignature(header: string | undefined | null): ParsedSignature | null {
  if (!header || header.length > 2048) return null;
  let timestamp: number | null = null;
  let eventId: string | null = null;
  const signatures: string[] = [];
  for (const part of header.split(",")) {
    const i = part.indexOf("=");
    if (i <= 0) return null;
    const key = part.slice(0, i).trim();
    const value = part.slice(i + 1).trim();
    if (key === "t" && /^\d{1,12}$/.test(value)) timestamp = Number(value);
    else if (key === "id" && /^[A-Za-z0-9-]{1,64}$/.test(value)) eventId = value;
    else if (key === "v1" && /^[0-9a-f]{64}$/.test(value)) signatures.push(value);
    else return null;
  }
  if (timestamp === null || eventId === null || signatures.length === 0) return null;
  return { timestamp, eventId, signatures };
}

/** 一度受け付けたイベントID（送信時刻の許容の幅のあいだだけ覚える）。 */
export class SeenEvents {
  private readonly seen = new Map<string, number>();

  /** まだ受け付けていなければ覚えて true、受け付け済みなら false。 */
  remember(eventId: string, nowSeconds: number): boolean {
    for (const [id, at] of this.seen) {
      if (nowSeconds - at > MAX_SKEW_SECONDS * 2) this.seen.delete(id);
    }
    if (this.seen.has(eventId)) return false;
    this.seen.set(eventId, nowSeconds);
    return true;
  }
}

/**
 * 署名を確かめる。正しければ true。
 * secrets には、いま受け付ける秘密（入れ替え中は新旧の両方）を渡す。
 */
export function verifySignature(
  header: string | undefined | null,
  body: string,
  secrets: string[],
  seen: SeenEvents,
  nowSeconds: number = Math.floor(Date.now() / 1000),
): boolean {
  if (Buffer.byteLength(body) > MAX_BODY_BYTES) return false;
  const parsed = parseSignature(header);
  if (!parsed) return false;
  if (Math.abs(nowSeconds - parsed.timestamp) > MAX_SKEW_SECONDS) return false;
  const usable = secrets.filter((s) => s.length > 0);
  if (usable.length === 0) return false;
  const signed = `${parsed.timestamp}.${parsed.eventId}.${body}`;
  let ok = false;
  for (const secret of usable) {
    const expected = Buffer.from(createHmac("sha256", secret).update(signed).digest("hex"), "utf8");
    for (const given of parsed.signatures) {
      const g = Buffer.from(given, "utf8");
      // 長さは正規表現で 64 にそろえてあるので、timingSafeEqual は例外を投げない
      if (g.length === expected.length && timingSafeEqual(g, expected)) ok = true;
    }
  }
  // 署名が正しいときだけ、イベントIDを覚える（偽のリクエストで正しいイベントを締め出させない）
  return ok && seen.remember(parsed.eventId, nowSeconds);
}
