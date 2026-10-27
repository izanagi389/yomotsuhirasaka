// 埋め込み: 文字の bigram をハッシュして次元に振り分け（符号付き）、長さ 1 に正規化する。
// 同じ入力には常に同じ値を返し、言葉が重なる文どうしは似たベクトルになる。

import { bigrams } from "./text.js";

const MODEL = /^mock-embed-(\d+)$/;
export const MAX_DIMENSIONS = 4096;

/** モデル名（`mock-embed-<次元数>`）から次元数を読む。知らないモデル名なら null。 */
export function embeddingDimensions(model: string): number | null {
  const m = MODEL.exec(model);
  if (!m) return null;
  const dims = Number(m[1]);
  return dims >= 1 && dims <= MAX_DIMENSIONS ? dims : null;
}

function fnv1a(text: string): number {
  let h = 0x811c9dc5;
  for (const byte of Buffer.from(text, "utf8")) {
    h ^= byte;
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h >>> 0;
}

export function embed(text: string, dimensions: number): number[] {
  const v = new Array<number>(dimensions).fill(0);
  for (const gram of bigrams(text)) {
    const h = fnv1a(gram);
    const sign = Math.floor(h / dimensions) % 2 === 0 ? 1 : -1;
    v[h % dimensions]! += sign;
  }
  const norm = Math.sqrt(v.reduce((s, x) => s + x * x, 0));
  return norm === 0 ? v : v.map((x) => x / norm);
}
