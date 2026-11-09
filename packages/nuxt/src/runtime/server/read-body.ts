// リクエストの本文を、上限まで読む（Nitro に依存しない）。`Content-Length` は省けるので当てにせず、届いた分を数える。

/** 本文を文字列で返す。上限を超えたら読むのをやめて null を返す。本文がなければ空文字列。 */
export async function readLimitedText(stream: ReadableStream<Uint8Array> | undefined | null, maxBytes: number): Promise<string | null> {
  if (!stream) return "";
  const reader = stream.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > maxBytes) {
      await reader.cancel();
      return null;
    }
    chunks.push(value);
  }
  return Buffer.concat(chunks).toString("utf8");
}
