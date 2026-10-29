// 質問応答の約束（回答の土台）: INV-QA-01・04・05・10・12・13・15（M4、09-qa-chatbot.md、13-security.md「チャットボットへの悪用」）
//
// - 回答は配信APIの GraphQL（Persisted Queries）で行う。まとめて返す Query `answer` と、判定先行のストリーミングの
//   Subscription `answerStream`（SSE）の両方で確かめる。回答APIは answer のスコープのキーで呼ぶ。
// - 舞台は専用のインスタンス（support/world.ts の createAnswerWorld）。擬似LLMの記録・障害・回答の組み立て方は
//   キーごとなので、このファイルのテストは1つのキーを共有し、テストごとに元に戻す（テストは順に流れる）。
// - 擬似LLMの既定の回答の組み立て方（overlap）は、質問と重なりの多い断片を出典にして、`[n]` 付きの回答を返す。
//   回答の中身を決めたいテストは `fixed` で回答（判定の先行宣言＋本文）を渡す。
//   CMSはプロンプトの断片に 1 から順に番号を付ける（format.json の `[n] 本文` の行）ので、`fixed` の回答は出典に 1 を使う。
// - 質問文は外部に送る前にマスクされ、10桁以上の数字の並びは `[number]` になる（Q169）。擬似LLMに届いた内容と
//   比べるテストの検索語には、数字を含めない（wordMarker）。

import { afterAll, afterEach, expect } from "vitest";
import { inv } from "./support/inv.js";
import { driver, type AnswerEvent, type AnswerResult } from "./support/driver.js";
import type { HttpResponse } from "./support/http.js";
import { doc, paragraph } from "./support/body.js";
import { chatRequests, citation, embeddingInputs, fixedAnswer, mockLlm } from "./support/mock-llm.js";
import { DEVNOTE_MARKER } from "./support/template.js";
import { createAnswerWorld, marker, sleep, type AnswerWorkspace, type AnswerWorld } from "./support/world.js";

let worldPromise: Promise<AnswerWorld> | null = null;
const world = (): Promise<AnswerWorld> => (worldPromise ??= createAnswerWorld());

afterEach(async () => {
  if (!worldPromise) return;
  const { llmKey } = await worldPromise;
  await mockLlm.clearFaults(llmKey);
  await mockLlm.resetStrategy(llmKey);
  await mockLlm.clearRequests(llmKey);
});

afterAll(async () => {
  if (worldPromise) await (await worldPromise).stop();
});

/** 数字を含まない一意な目印（マスクで変わらないようにする）。 */
const wordMarker = (label: string): string => marker(label).replace(/\d/g, (d) => "ghijklmnop"[Number(d)]!);

/** 記事を公開し、検索に出る（埋め込みができる）まで待つ。 */
async function publishSearchable(ws: AnswerWorkspace, word: string, text: string, fields?: Record<string, unknown>): Promise<string> {
  const { cms } = await world();
  const { id } = await cms.createEntry(ws.editorSession, ws.id, { title: marker("qa-title"), body: doc(paragraph(text)), fields });
  await cms.publishEntry(ws.editorSession, ws.id, id);
  const deadline = Date.now() + 120_000;
  for (;;) {
    const r = await cms.search(ws.id, ws.readKey.secret, ws.searchQuery, { text: word, first: 50 });
    if (r.hits.some((h) => h.entryId === id)) return id;
    if (Date.now() > deadline) throw new Error(`公開した記事が検索に出ない（埋め込みができない）: ${id}`);
    await sleep(1);
  }
}

async function askRaw(ws: AnswerWorkspace, question: string, stream: boolean): Promise<HttpResponse> {
  const { cms } = await world();
  return cms.answerRaw(ws.id, ws.answerKey.secret, stream ? ws.answerStreamQuery : ws.answerQuery, { question });
}

/** まとめて返す回答。エラーなら、その内容で失敗する。 */
async function ask(ws: AnswerWorkspace, question: string): Promise<AnswerResult> {
  const { cms } = await world();
  const res = await askRaw(ws, question, false);
  const r = cms.parseAnswer(res);
  expect(r, `回答が返らない（${res.status}）: ${JSON.stringify(cms.answerErrors(res))}`).not.toBeNull();
  return r!;
}

/** ストリーミングの回答のイベント。最終判定が1つ、最後に来ることも確かめる。 */
async function askStream(ws: AnswerWorkspace, question: string): Promise<{ events: AnswerEvent[]; final: Extract<AnswerEvent, { type: "final" }> }> {
  const { cms } = await world();
  const res = await askRaw(ws, question, true);
  const events = cms.parseAnswerStream(res);
  expect(cms.answerErrors(res), `ストリーミングの回答がエラーになった（${res.status}）`).toEqual([]);
  const finals = events.filter((e) => e.type === "final");
  expect(finals.length, "最終判定のイベントが1つでない").toBe(1);
  expect(events.at(-1)?.type, "最終判定のイベントが最後に来ない").toBe("final");
  return { events, final: finals[0] as Extract<AnswerEvent, { type: "final" }> };
}

const deltas = (events: AnswerEvent[]): string[] => events.flatMap((e) => (e.type === "delta" ? [e.text] : []));
const chars = (s: string): number => [...s].length;

inv(
  "inv_qa_01",
  "引用が1つもない回答は、「情報なし」に差し替えられて返される",
  async () => {
    const { llmKey } = await world();
    const ws = await (await world()).workspace("qa01");
    const word = wordMarker("qaone");
    await publishSearchable(ws, word, `${word} は、引用の検証を確かめるための記事です。回答には出典の番号が付きます。`);
    const question = `${word} とは何ですか`;

    // 対照: 引用のある回答は、そのまま返る
    const normal = await ask(ws, question);
    expect(normal.status, "引用のある回答が返らない（擬似LLMの既定の組み立て方）").toBe("answered");
    expect(normal.sources.length).toBeGreaterThan(0);

    // 1. 擬似LLMが本文から引用記号を取り除く（宣言は「答えられる」のまま）
    await mockLlm.setFaults(llmKey, [{ kind: "noCitations", endpoint: "chat" }]);
    const mockBody = (r: AnswerResult) => r.text.includes(word);
    const replaced = await ask(ws, question);
    expect(replaced.status, "引用のない回答が「情報なし」に差し替えられない（まとめて）").toBe("noInformation");
    expect(mockBody(replaced), "引用のない回答の本文が返った（まとめて）").toBe(false);
    expect(replaced.sources, "「情報なし」に出典が付いている").toEqual([]);
    const streamed = await askStream(ws, question);
    expect(streamed.final.status, "引用のない回答が「情報なし」に差し替えられない（ストリーミングの最終判定）").toBe("noInformation");
    expect(mockBody(streamed.final), "引用のない回答の本文が、最終判定で返った").toBe(false);
    expect(streamed.final.sources).toEqual([]);

    // 2. 回答をテストが決める: 宣言は正しいが、本文に引用が1つもない
    await mockLlm.clearFaults(llmKey);
    const uncited = `引用のない本文 ${wordMarker("uncited")}`;
    await mockLlm.setStrategy(llmKey, { kind: "fixed", content: fixedAnswer([1], uncited) });
    const fixed = await ask(ws, question);
    expect(fixed.status, "引用のない回答が「情報なし」に差し替えられない（決めた回答・まとめて）").toBe("noInformation");
    expect(fixed.text, "引用のない回答の本文が返った（決めた回答）").not.toContain(uncited);
    const fixedStream = await askStream(ws, question);
    expect(fixedStream.final.status).toBe("noInformation");
    expect(fixedStream.final.text).not.toContain(uncited);

    // 対照: LLMは呼ばれていた（呼ばずに「情報なし」にしたのではなく、回答を検証して差し替えた）
    expect(chatRequests(await mockLlm.requests(llmKey)).length, "LLMが呼ばれていない").toBeGreaterThanOrEqual(4);
  },
  300_000,
);

inv(
  "inv_qa_04",
  "LLMが「答えられない」と宣言した場合や、実在しない出典番号を宣言した場合は、本文が配信される前に「情報なし」が返される",
  async () => {
    const { llmKey } = await world();
    const ws = await (await world()).workspace("qa04");
    const word = wordMarker("qafour");
    await publishSearchable(ws, word, `${word} は、判定の先行宣言を確かめるための記事です。宣言を確かめてから本文を配信します。`);
    const question = `${word} とは何ですか`;

    // 対照: 答えられる回答では、判定（答えられる）の後に本文の断片が配信される
    const normal = await askStream(ws, question);
    expect(normal.final.status, "答えられる回答が返らない").toBe("answered");
    expect(normal.events[0]?.type, "最初のイベントが判定でない").toBe("verdict");
    expect(normal.events[0]?.type === "verdict" && normal.events[0].answerable).toBe(true);
    expect(deltas(normal.events).length, "本文の断片が配信されない").toBeGreaterThan(0);

    /** 本文の断片が1つも配信されず、最終判定が「情報なし」で、宣言の後に書かれた本文が返らない。 */
    const expectNoInformation = async (label: string, forbidden: string | null) => {
      const s = await askStream(ws, question);
      expect(deltas(s.events), `${label}: 本文が配信された（ストリーミング）`).toEqual([]);
      for (const e of s.events) {
        if (e.type === "verdict") expect(e.answerable, `${label}: 判定が「答えられる」になった`).toBe(false);
      }
      expect(s.final.status, `${label}: 最終判定が「情報なし」でない`).toBe("noInformation");
      const whole = await ask(ws, question);
      expect(whole.status, `${label}: 「情報なし」が返らない（まとめて）`).toBe("noInformation");
      if (forbidden !== null) {
        expect(s.final.text, `${label}: 宣言の後の本文が返った（ストリーミング）`).not.toContain(forbidden);
        expect(whole.text, `${label}: 宣言の後の本文が返った（まとめて）`).not.toContain(forbidden);
      }
    };

    // 1. 「答えられない」と宣言する（宣言の後に本文を書いても、配信しない）
    const afterNo = `答えられないと宣言した後の本文 ${wordMarker("afterno")} ${citation(1)}`;
    await mockLlm.setStrategy(llmKey, { kind: "fixed", content: fixedAnswer([], afterNo) });
    await expectNoInformation("「答えられない」の宣言", afterNo);

    // 2. 実在しない出典番号を宣言する（擬似LLMの障害: 断片にない番号を宣言し、本文でも使う）
    await mockLlm.resetStrategy(llmKey);
    await mockLlm.setFaults(llmKey, [{ kind: "unknownSource", endpoint: "chat" }]);
    await expectNoInformation("実在しない出典番号の宣言（障害）", null);

    // 3. 実在しない出典番号だけを宣言する（回答をテストが決める）
    await mockLlm.clearFaults(llmKey);
    const unknown = `実在しない出典を使う本文 ${wordMarker("unknown")} ${citation(99)}`;
    await mockLlm.setStrategy(llmKey, { kind: "fixed", content: fixedAnswer([99], unknown) });
    await expectNoInformation("実在しない出典番号の宣言（決めた回答）", unknown);
  },
  300_000,
);

inv(
  "inv_qa_05",
  "訪問者向けの回答の予算を超えた場合は、LLMを呼ばずに予算切れを示す専用のエラーコードを返す",
  async () => {
    const w = await world();
    const { cms, llmKey } = w;
    // 回答の予算の天井が 1 トークン（どんな回答も予算を超える）のワークスペースと、十分に大きいワークスペース
    const poor = await w.workspace("qa05-poor", { answerCeiling: 1 });
    const rich = await w.workspace("qa05-rich");
    const word = wordMarker("qafive");
    const text = `${word} は、回答の予算を確かめるための記事です。`;
    await publishSearchable(poor, word, text);
    await publishSearchable(rich, word, text);
    const question = `${word} とは何ですか`;

    // 予算を超える: まとめて・ストリーミングのどちらも、予算切れのエラーコードが返り、LLM（チャット）は呼ばれない
    await mockLlm.clearRequests(llmKey);
    for (const stream of [false, true]) {
      const label = stream ? "ストリーミング" : "まとめて";
      const res = await askRaw(poor, question, stream);
      expect(cms.answerErrors(res).map((e) => e.code), `予算切れのエラーコードが返らない（${label}）`).toContain("ANSWER_BUDGET_EXHAUSTED");
      expect(stream ? cms.parseAnswerStream(res).filter((e) => e.type !== "final") : [], `予算切れなのに回答のイベントが届いた（${label}）`).toEqual([]);
      if (!stream) expect(cms.parseAnswer(res), "予算切れなのに回答が返った").toBeNull();
    }
    expect(chatRequests(await mockLlm.requests(llmKey)), "予算を超えているのに、LLMが呼ばれた").toEqual([]);

    // 対照: 予算の範囲では、LLMが呼ばれて回答が返る
    const answered = await ask(rich, question);
    expect(answered.status).toBe("answered");
    expect(chatRequests(await mockLlm.requests(llmKey)).length, "予算の範囲なのに、LLMが呼ばれない").toBeGreaterThan(0);
  },
  300_000,
);

inv(
  "inv_qa_10",
  "`devNote` の内容は、LLMへのリクエストに含まれない",
  async () => {
    const { llmKey } = await world();
    const ws = await (await world()).workspace("qa10");
    // devNote は記事のコンテンツタイプと summary フィールドに付いている（support/template.ts）
    const word = wordMarker("qaten");
    await publishSearchable(ws, word, `${word} は、devNote が外に出ないことを確かめるための記事です。`, { summary: `${word} の要約` });
    const question = `${word} とは何ですか`;
    await ask(ws, question);
    await askStream(ws, question);

    const sent = JSON.stringify(chatRequests(await mockLlm.requests(llmKey)).map((r) => r.body));
    // 対照: LLMへのリクエストは届いていて、記事の断片を含む
    expect(sent, "LLMへのリクエストに、記事の断片が含まれない").toContain(word);
    expect(sent, "LLMへのリクエストに devNote の内容が含まれる").not.toContain(DEVNOTE_MARKER);
  },
  300_000,
);

inv(
  "inv_qa_12",
  "上限を超える長さの質問は拒否される",
  async () => {
    const { cms, llmKey } = await world();
    const ws = await (await world()).workspace("qa12");
    const word = wordMarker("qatwelve");
    await publishSearchable(ws, word, `${word} は、質問の長さの上限を確かめるための記事です。`);
    const max = driver.answerLimits.questionMaxChars;
    /** 指定した文字数の質問（目印を含む）。 */
    const questionOf = (n: number) => `${word} ${"質問の長さ".repeat(n)}`.slice(0, n);
    expect(chars(questionOf(max + 1))).toBe(max + 1);

    // 上限を1文字超える質問は、まとめて・ストリーミングのどちらも拒否され、LLMは呼ばれない
    await mockLlm.clearRequests(llmKey);
    for (const stream of [false, true]) {
      const label = stream ? "ストリーミング" : "まとめて";
      const res = await askRaw(ws, questionOf(max + 1), stream);
      expect(cms.answerErrors(res).map((e) => e.code), `上限を超える質問が拒否されない（${label}）`).toContain("QUESTION_TOO_LONG");
      if (stream) expect(cms.parseAnswerStream(res), "上限を超える質問に、回答のイベントが届いた").toEqual([]);
      else expect(cms.parseAnswer(res), "上限を超える質問に、回答が返った").toBeNull();
    }
    expect(chatRequests(await mockLlm.requests(llmKey)), "拒否した質問で、LLMが呼ばれた").toEqual([]);

    // 対照: 上限ちょうどの質問は受け付ける
    await ask(ws, questionOf(max));
    await askStream(ws, questionOf(max));
  },
  300_000,
);

inv(
  "inv_qa_13",
  "回答の長さは上限を超えない",
  async () => {
    const { llmKey } = await world();
    const ws = await (await world()).workspace("qa13");
    const word = wordMarker("qathirteen");
    await publishSearchable(ws, word, `${word} は、回答の長さの上限を確かめるための記事です。`);
    const question = `${word} とは何ですか`;
    const max = driver.answerLimits.answerMaxChars;

    // 対照: 短い回答は打ち切られない
    await mockLlm.setStrategy(llmKey, { kind: "fixed", content: fixedAnswer([1], `短い回答です ${citation(1)}`) });
    const short = await ask(ws, question);
    expect(short.status).toBe("answered");
    expect(short.truncated, "短い回答が打ち切られた").toBe(false);

    // LLMが上限の倍を超える長さの回答を返す（引用は先頭に置き、打ち切っても残るようにする）
    const long = `${citation(1)} ${"とても長い回答の文です。".repeat(Math.ceil((max * 2) / 12))}`;
    expect(chars(long)).toBeGreaterThan(max * 2);
    await mockLlm.setStrategy(llmKey, { kind: "fixed", content: fixedAnswer([1], long) });

    const whole = await ask(ws, question);
    expect(whole.status, "長い回答が返らない（まとめて）").toBe("answered");
    expect(chars(whole.text), "回答の長さが上限を超えた（まとめて）").toBeLessThanOrEqual(max);
    expect(whole.truncated, "打ち切ったことが示されない（まとめて）").toBe(true);

    const s = await askStream(ws, question);
    expect(s.final.status, "長い回答が返らない（ストリーミング）").toBe("answered");
    expect(chars(deltas(s.events).join("")), "配信した本文の長さが上限を超えた（ストリーミング）").toBeLessThanOrEqual(max);
    expect(chars(s.final.text), "最終判定の本文の長さが上限を超えた").toBeLessThanOrEqual(max);
    expect(s.final.truncated, "打ち切ったことが示されない（ストリーミング）").toBe(true);
  },
  300_000,
);

inv(
  "inv_qa_15",
  "外部のLLMへのリクエストと埋め込みの作成のリクエストには、訪問者の質問文のメールアドレスや電話番号などの定型パターンがマスクされた形でしか含まれない",
  async () => {
    const { llmKey } = await world();
    const ws = await (await world()).workspace("qa15");
    const word = wordMarker("qafifteen");
    await publishSearchable(ws, word, `${word} は、質問文のマスクを確かめるための記事です。`);
    const email = `${wordMarker("visitor").toLowerCase()}@example.com`;
    const phone = "090-4821-7356";
    const question = `${word} について ${email} か ${phone} に連絡がほしい`;

    await mockLlm.clearRequests(llmKey);
    await ask(ws, question);
    await askStream(ws, question);
    const requests = await mockLlm.requests(llmKey);
    const chats = JSON.stringify(chatRequests(requests).map((r) => r.body));
    const embeddings = JSON.stringify(embeddingInputs(requests));

    // 対照: 質問はLLMにも埋め込みの作成にも届いている（目印は数字を含まないので、マスクで変わらない）
    expect(chats, "質問がLLMに届いていない").toContain(word);
    expect(embeddings, "質問が埋め込みの作成に届いていない").toContain(word);
    for (const [label, sent] of [
      ["LLMへのリクエスト", chats],
      ["埋め込みの作成のリクエスト", embeddings],
    ] as const) {
      expect(sent, `${label}に、メールアドレスがそのまま含まれる`).not.toContain(email);
      expect(sent, `${label}に、電話番号がそのまま含まれる`).not.toContain(phone);
      expect(sent, `${label}に、電話番号の数字がそのまま含まれる`).not.toContain(phone.replace(/-/g, ""));
    }
  },
  300_000,
);
