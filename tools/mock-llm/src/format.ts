// 質問応答のプロンプトと回答の形（format.json）を読み書きする。

import format from "./format.json" with { type: "json" };

export const FORMAT = format;

const fragmentLine = new RegExp(format.fragmentLine, "u");
const citation = new RegExp(format.citation, "gu");

/** プロンプトの中の断片（`[n] 本文` の行）。同じ番号が2回あれば、先のものを使う。 */
export function parseFragments(text: string): Map<number, string> {
  const out = new Map<number, string>();
  for (const line of text.split(/\r?\n/u)) {
    const m = fragmentLine.exec(line);
    if (m && !out.has(Number(m[1]))) out.set(Number(m[1]), m[2]!.trim());
  }
  return out;
}

/** 断片の行を取り除いた文字列。 */
export function withoutFragments(text: string): string {
  return text
    .split(/\r?\n/u)
    .filter((line) => !fragmentLine.test(line))
    .join("\n")
    .trim();
}

/** 判定の先行宣言の行。出典が空なら「答えられない」。 */
export function declaration(sources: readonly number[]): string {
  return sources.length === 0
    ? format.notAnswerableDeclaration
    : format.answerableDeclaration.replace("{sources}", sources.join(format.sourceSeparator));
}

export function cite(n: number): string {
  return `[${n}]`;
}

export function removeCitations(text: string): string {
  return text.replace(citation, "").replace(/ {2,}/gu, " ").replace(/ +([。．.!?！？])/gu, "$1").trim();
}
