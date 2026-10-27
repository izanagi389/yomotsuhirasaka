// チャットの回答を、プロンプトの中の断片から決まった手順で組み立てる。
// 回答の1行目は判定の先行宣言、2行目からが本文（format.json）。

import { cite, declaration, FORMAT, parseFragments, removeCitations, withoutFragments } from "./format.js";
import { bigrams } from "./text.js";

export interface ChatMessage {
  role: string;
  content: string | ReadonlyArray<{ type: string; text?: string }>;
}

/** 回答の組み立て方（API キーごとにテストから選べる）。 */
export type Strategy =
  /** 質問との bigram の重なりが多い順に、最大 maxSources 個を出典にする。重なりが minShared 未満の断片は使わない。 */
  | { kind: "overlap"; maxSources?: number; minShared?: number }
  /** 番号の小さい順に、最大 n 個を出典にする。 */
  | { kind: "firstN"; n?: number }
  /** テストが決めた回答（宣言の行を含む全体）をそのまま返す。回答の中身を変える障害は効かない。 */
  | { kind: "fixed"; content: string };

export const DEFAULT_STRATEGY: Strategy = { kind: "overlap", maxSources: 2, minShared: 2 };

/** 回答の中身を変える障害。 */
export interface ContentFaults {
  /** 本文から引用記号を取り除く（宣言はそのまま）。 */
  noCitations?: boolean;
  /** 実在しない出典番号を宣言し、本文でも使う。 */
  unknownSource?: boolean;
  /** 宣言していない番号を本文で使う。 */
  undeclaredCitation?: boolean;
}

export function messageText(message: ChatMessage): string {
  return typeof message.content === "string"
    ? message.content
    : message.content.map((part) => (part.type === "text" ? (part.text ?? "") : "")).join("");
}

function firstSentence(text: string): string {
  // 全角の句点は続く空白を求めない（日本語の文は空白で区切らない）
  const m = /^(.+?(?:[。．！？]|[.!?](?=\s|$)))/u.exec(text);
  return (m ? m[1]! : text).trim();
}

function chooseSources(question: string, fragments: Map<number, string>, strategy: Strategy): number[] {
  const numbers = [...fragments.keys()].sort((a, b) => a - b);
  if (strategy.kind === "firstN") return numbers.slice(0, strategy.n ?? 2);
  const maxSources = strategy.kind === "overlap" ? (strategy.maxSources ?? 2) : 2;
  const minShared = strategy.kind === "overlap" ? (strategy.minShared ?? 2) : 2;
  const asked = new Set(bigrams(question));
  return numbers
    .map((n) => ({ n, shared: new Set(bigrams(fragments.get(n)!).filter((g) => asked.has(g))).size }))
    .filter((s) => s.shared >= minShared)
    .sort((a, b) => b.shared - a.shared || a.n - b.n)
    .slice(0, maxSources)
    .map((s) => s.n)
    .sort((a, b) => a - b);
}

export function composeAnswer(messages: readonly ChatMessage[], strategy: Strategy, faults: ContentFaults = {}): string {
  if (strategy.kind === "fixed") return strategy.content;
  const fragments = parseFragments(messages.map(messageText).join("\n"));
  const lastUser = [...messages].reverse().find((m) => m.role === "user");
  const question = lastUser ? withoutFragments(messageText(lastUser)) : "";
  const sources = chooseSources(question, fragments, strategy);
  const declared = [...sources];
  let body =
    sources.length === 0
      ? FORMAT.notAnswerableBody
      : sources.map((n) => `${firstSentence(fragments.get(n)!)} ${cite(n)}`).join(" ");

  const highest = Math.max(0, ...fragments.keys());
  if (faults.unknownSource) {
    const unknown = highest + 1;
    if (declared.length === 0) body = "";
    declared.push(unknown);
    body = `${body} ${cite(unknown)}`.trim();
  }
  if (faults.undeclaredCitation) {
    const unused = [...fragments.keys()].sort((a, b) => a - b).find((n) => !declared.includes(n));
    const extra = unused ?? Math.max(highest, ...declared) + 1;
    body = `${body} ${cite(extra)}`;
  }
  if (faults.noCitations) body = removeCitations(body);
  return `${declaration(declared)}\n${body}`;
}
