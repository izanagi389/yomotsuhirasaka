// 文字列の共通の処理（トークン数の数え方と、文字の bigram）。

/** トークン数: 文字数（Unicode のコードポイント）÷ 4 の切り上げ。少なくとも 1。 */
export function countTokens(text: string): number {
  return Math.max(1, Math.ceil([...text].length / 4));
}

/**
 * 文字の bigram（隣り合う2文字の組）。NFKC で正規化して小文字にし、連続する空白は1つにまとめる。
 * 1文字だけの文字列は、その1文字を返す。
 */
export function bigrams(text: string): string[] {
  const chars = [...text.normalize("NFKC").toLowerCase().replace(/\s+/gu, " ").trim()];
  if (chars.length < 2) return chars.length === 1 ? [chars[0]!] : [];
  const out: string[] = [];
  for (let i = 0; i < chars.length - 1; i++) out.push(chars[i]! + chars[i + 1]!);
  return out;
}
