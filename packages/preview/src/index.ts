// ライブプレビューの、管理画面とサイトの間のメッセージ（03-content-and-editing.md「プレビュー（Q72）」、
// 13-security.md「ライブプレビュー」、INV-PRV-03）。
//
// - サイト → 管理画面: `yomi:preview:ready`（プレビューのページが読み込まれた）
// - 管理画面 → サイト: `yomi:preview:refresh`（下書きが保存されたので、CMSから取り直してほしい）
// メッセージには記事の中身を載せない。サイトは知らせを受けたら、プレビュー用トークンでCMSから下書きを取り直す
// （中身は必ずCMSの検証を通る）。
//
// 受け手は、送り元のオリジンが設定した一覧と完全に一致し、送り元の窓が期待した窓（管理画面なら iframe、
// サイトなら親の窓）のときだけ受け付ける。オリジンの比較は文字列の完全一致（似たドメインや別のスキーム・ポートは別物）。

export const READY = "yomi:preview:ready";
export const REFRESH = "yomi:preview:refresh";

export type PreviewMessage = { type: typeof READY } | { type: typeof REFRESH };

/** 受け取ったメッセージ（`MessageEvent` のうち、判定に使うところ）。 */
export interface IncomingMessage {
  origin: string;
  source: unknown;
  data: unknown;
}

/** 受け付ける相手。 */
export interface ExpectedSender {
  /** 受け付けるオリジン（`https://example.com` の形）。 */
  origins: readonly string[];
  /** 受け付ける窓（管理画面ではプレビューの iframe の contentWindow、サイトでは window.parent）。 */
  source: unknown;
}

/** 受け付けてよいメッセージなら、その中身。そうでなければ null。 */
export function acceptPreviewMessage(event: IncomingMessage, expected: ExpectedSender): PreviewMessage | null {
  if (expected.source === null || expected.source === undefined || event.source !== expected.source) return null;
  if (event.origin === "null" || !expected.origins.includes(event.origin)) return null;
  const data = event.data;
  if (typeof data !== "object" || data === null) return null;
  const type = (data as { type?: unknown }).type;
  if (type === READY) return { type: READY };
  if (type === REFRESH) return { type: REFRESH };
  return null;
}
