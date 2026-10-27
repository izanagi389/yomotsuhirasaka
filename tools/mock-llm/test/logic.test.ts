import { describe, expect, test } from "vitest";
import { composeAnswer, DEFAULT_STRATEGY, type ChatMessage } from "../src/chat.js";
import { embed, embeddingDimensions } from "../src/embed.js";
import { FORMAT, parseFragments } from "../src/format.js";
import { countTokens } from "../src/text.js";

const cosine = (a: number[], b: number[]) => a.reduce((s, x, i) => s + x * b[i]!, 0);

describe("トークン数", () => {
  test("文字数（コードポイント）÷ 4 の切り上げで、少なくとも 1", () => {
    expect(countTokens("a")).toBe(1);
    expect(countTokens("abcd")).toBe(1);
    expect(countTokens("abcde")).toBe(2);
    expect(countTokens("日本語の文章です")).toBe(2);
    expect(countTokens("😀😀😀😀😀")).toBe(2);
  });
});

describe("埋め込み", () => {
  test("次元数はモデル名で決まり、知らないモデル名は null", () => {
    expect(embeddingDimensions("mock-embed-64")).toBe(64);
    expect(embeddingDimensions("mock-embed-3")).toBe(3);
    expect(embeddingDimensions("mock-embed-0")).toBeNull();
    expect(embeddingDimensions("mock-embed-5000")).toBeNull();
    expect(embeddingDimensions("text-embedding-3-small")).toBeNull();
  });

  test("同じ入力には同じ値を返し、長さは 1", () => {
    const a = embed("PostgreSQL の全文検索", 64);
    expect(embed("PostgreSQL の全文検索", 64)).toEqual(a);
    expect(a).toHaveLength(64);
    expect(cosine(a, a)).toBeCloseTo(1, 10);
  });

  test("言葉が重なる文どうしは、重ならない文より似ている", () => {
    const query = embed("PostgreSQLで日本語の全文検索をする方法", 64);
    const near = embed("PostgreSQLの日本語の全文検索の設定", 64);
    const far = embed("週末に作るカレーの献立", 64);
    expect(cosine(query, near)).toBeGreaterThan(cosine(query, far) + 0.2);
  });
});

const prompt = (question: string, fragments: string[]): ChatMessage[] => [
  { role: "system", content: `Answer from the fragments.\n${fragments.map((f, i) => `[${i + 1}] ${f}`).join("\n")}` },
  { role: "user", content: question },
];

const FRAGMENTS = [
  "週末に作るカレーは、玉ねぎをよく炒めます。辛さは好みで。",
  "PostgreSQLでは pg_bigm を使うと日本語の全文検索ができます。インデックスも作れます。",
  "pg_bigm のインデックスは、日本語の全文検索を速くします。",
];

describe("チャットの回答", () => {
  test("断片の行を読む（同じ番号は先のもの）", () => {
    expect([...parseFragments("x\n[2] b\n[1] a\n[2] c").entries()]).toEqual([
      [2, "b"],
      [1, "a"],
    ]);
  });

  test("既定（overlap）では、質問との重なりが多い断片を最大2つ出典にし、宣言と本文の引用がそろう", () => {
    const answer = composeAnswer(prompt("日本語の全文検索をするには？", FRAGMENTS), DEFAULT_STRATEGY);
    const [first, ...rest] = answer.split("\n");
    expect(first).toBe("ANSWERABLE: yes; SOURCES: 2,3");
    expect(first).toMatch(new RegExp(FORMAT.declarationLine, "u"));
    expect(rest.join("\n")).toBe("PostgreSQLでは pg_bigm を使うと日本語の全文検索ができます。 [2] pg_bigm のインデックスは、日本語の全文検索を速くします。 [3]");
  });

  test("重なる断片がなければ「答えられない」", () => {
    const answer = composeAnswer(prompt("量子コンピューターとは", FRAGMENTS), DEFAULT_STRATEGY);
    expect(answer).toBe(`${FORMAT.notAnswerableDeclaration}\n${FORMAT.notAnswerableBody}`);
    expect(composeAnswer(prompt("日本語の全文検索", []), DEFAULT_STRATEGY).split("\n")[0]).toBe(FORMAT.notAnswerableDeclaration);
  });

  test("同じ入力には同じ回答を返す", () => {
    const p = prompt("日本語の全文検索をするには？", FRAGMENTS);
    expect(composeAnswer(p, DEFAULT_STRATEGY)).toBe(composeAnswer(p, DEFAULT_STRATEGY));
  });

  test("firstN は番号の小さい順、fixed はそのまま返す", () => {
    expect(composeAnswer(prompt("なんでも", FRAGMENTS), { kind: "firstN", n: 1 }).split("\n")[0]).toBe("ANSWERABLE: yes; SOURCES: 1");
    expect(composeAnswer(prompt("なんでも", FRAGMENTS), { kind: "fixed", content: "ANSWERABLE: yes; SOURCES: 1\nX [3]" })).toBe(
      "ANSWERABLE: yes; SOURCES: 1\nX [3]",
    );
  });

  test("障害: 引用なし・実在しない出典・宣言にない番号", () => {
    const p = prompt("日本語の全文検索をするには？", FRAGMENTS);
    const noCitations = composeAnswer(p, DEFAULT_STRATEGY, { noCitations: true }).split("\n");
    expect(noCitations[0]).toBe("ANSWERABLE: yes; SOURCES: 2,3");
    expect(noCitations[1]).not.toMatch(/\[\d+\]/u);

    const unknown = composeAnswer(p, DEFAULT_STRATEGY, { unknownSource: true }).split("\n");
    expect(unknown[0]).toBe("ANSWERABLE: yes; SOURCES: 2,3,4");
    expect(unknown[1]).toContain("[4]");

    const undeclared = composeAnswer(p, DEFAULT_STRATEGY, { undeclaredCitation: true }).split("\n");
    expect(undeclared[0]).toBe("ANSWERABLE: yes; SOURCES: 2,3");
    expect(undeclared[1]).toMatch(/\[1\]$/u);
  });
});
