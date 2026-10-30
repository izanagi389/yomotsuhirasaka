// 回答キャッシュの約束: INV-QA-06〜09（M4、09-qa-chatbot.md「回答キャッシュ」、06-corrections-and-freshness.md）
//
// - 舞台は専用のインスタンス（support/world.ts の createAnswerWorld）。擬似LLMの既定の回答の組み立て方（overlap）で、
//   質問と重なりの多い断片を出典にした回答が返る。回答のクエリは、キャッシュから返したか（cached）も選ぶ形で登録する。
// - キャッシュから返したかは、応答の cached と、擬似LLMへのチャットの依頼が増えないことの両方で確かめる。
// - QA-08（平文を保存しない）と QA-09（確認期限をまたぐ）は、CLAUDE.md の例外として driver.storage でDBに直接つなぐ。
// - 質問文は外部に送る前にマスクされるので、検索語には数字を含めない（wordMarker）。

import { afterAll, afterEach, expect } from "vitest";
import { inv } from "./support/inv.js";
import { driver, type AnswerEvent, type AnswerResult } from "./support/driver.js";
import { doc, paragraph } from "./support/body.js";
import { chatRequests, mockLlm } from "./support/mock-llm.js";
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

/** 記事を作って公開し、その記事を出典にした回答が返るまで待つ（埋め込みができるまで）。回答の出典の版を返す。 */
async function publishAnswerable(ws: AnswerWorkspace, question: string, text: string): Promise<{ id: string; versionId: string }> {
  const { cms } = await world();
  const { id } = await cms.createEntry(ws.editorSession, ws.id, { title: marker("qa-title"), body: doc(paragraph(text)) });
  await cms.publishEntry(ws.editorSession, ws.id, id);
  return waitAnswerable(ws, question, id);
}

/** 記事 `id` を出典にした回答が返るまで待つ。`notVersion` を渡すと、その版でない版を出典にしたものを待つ。 */
async function waitAnswerable(ws: AnswerWorkspace, question: string, id: string, notVersion?: string): Promise<{ id: string; versionId: string }> {
  const deadline = Date.now() + 120_000;
  for (;;) {
    const r = await ask(ws, question);
    const source = r.sources.find((s) => s.entryId === id && s.versionId !== notVersion);
    if (r.status === "answered" && source) return { id, versionId: source.versionId };
    if (Date.now() > deadline) throw new Error(`公開した記事を出典にした回答が返らない（埋め込みができない）: ${id}`);
    await sleep(1);
  }
}

/** まとめて返す回答。エラーなら、その内容で失敗する。 */
async function ask(ws: AnswerWorkspace, question: string): Promise<AnswerResult> {
  const { cms } = await world();
  const res = await cms.answerRaw(ws.id, ws.answerKey.secret, ws.answerQuery, { question });
  const r = cms.parseAnswer(res);
  expect(r, `回答が返らない（${res.status}）: ${JSON.stringify(cms.answerErrors(res))}`).not.toBeNull();
  return r!;
}

/** ストリーミングの回答のイベント。 */
async function askStream(ws: AnswerWorkspace, question: string): Promise<{ events: AnswerEvent[]; final: Extract<AnswerEvent, { type: "final" }> }> {
  const { cms } = await world();
  const res = await cms.answerRaw(ws.id, ws.answerKey.secret, ws.answerStreamQuery, { question });
  const events = cms.parseAnswerStream(res);
  expect(cms.answerErrors(res), `ストリーミングの回答がエラーになった（${res.status}）`).toEqual([]);
  const final = events.at(-1);
  expect(final?.type, "最終判定のイベントが最後に来ない").toBe("final");
  return { events, final: final as Extract<AnswerEvent, { type: "final" }> };
}

const chatCount = async (): Promise<number> => {
  const { llmKey } = await world();
  return chatRequests(await mockLlm.requests(llmKey)).length;
};

/** キャッシュから返した（LLMを呼ばずに、cached が true で返った）ことを確かめる。 */
async function expectCached(ws: AnswerWorkspace, question: string, what: string): Promise<AnswerResult> {
  const before = await chatCount();
  const r = await ask(ws, question);
  expect(r.status, `${what}: 回答が返らない`).toBe("answered");
  expect(r.cached, `${what}: キャッシュから返らない`).toBe(true);
  expect(await chatCount(), `${what}: キャッシュがあるのに、LLMが呼ばれた`).toBe(before);
  return r;
}

inv(
  "inv_qa_06",
  "引用元の記事が非公開になった、または版が変わった回答キャッシュは返されない",
  async () => {
    const { cms } = await world();
    const ws = await (await world()).workspace("qa06", { cached: true });
    const word = wordMarker("qasix");
    const question = `${word} とは何ですか`;
    const v1 = await publishAnswerable(ws, question, `${word} は、回答キャッシュを確かめるための記事です。`);

    // 対照: 同じ質問は、キャッシュから返る（空白・大文字小文字の違いは正規化される）。ストリーミングでも同じ
    const hit = await expectCached(ws, question, "同じ質問");
    expect(hit.sources.map((s) => s.versionId)).toContain(v1.versionId);
    await expectCached(ws, `  ${question.toUpperCase()}　`, "空白と大文字小文字だけが違う質問");
    const streamed = await askStream(ws, question);
    expect(streamed.final.status).toBe("answered");
    expect(streamed.final.cached, "ストリーミングで、キャッシュから返らない").toBe(true);

    // 1. 版が変わった: キャッシュは返されない（通常の更新では、新しい版の検索用データができるまで旧版で答え続けてよい。Q31）
    const updated = `${wordMarker("qasixnew")} ${word} は、版を変えた記事です。`;
    await cms.saveDraft(ws.editorSession, ws.id, v1.id, { body: doc(paragraph(updated)) });
    await cms.publishEntry(ws.editorSession, ws.id, v1.id);
    for (const r of [await ask(ws, question), (await askStream(ws, question)).final]) {
      expect(r.cached, "版が変わった回答キャッシュが返った").not.toBe(true);
    }

    // 対照: 新しい版で答えた回答は、キャッシュから返る
    const v2 = await waitAnswerable(ws, question, v1.id, v1.versionId);
    expect(v2.versionId).not.toBe(v1.versionId);
    await expectCached(ws, question, "新しい版の回答");

    // 2. 非公開になった: キャッシュは返されない（その記事を出典にした回答も返らない）
    await cms.unpublishEntry(ws.editorSession, ws.id, v1.id);
    for (const r of [await ask(ws, question), (await askStream(ws, question)).final]) {
      expect(r.cached, "非公開になった記事を引用した回答キャッシュが返った").not.toBe(true);
      expect(r.sources.map((s) => s.entryId), "非公開になった記事が出典に含まれる").not.toContain(v1.id);
    }
  },
  300_000,
);

inv(
  "inv_qa_07",
  "「訂正として公開」した記事の旧版は、その直後から回答の根拠にも、回答キャッシュにも使われない",
  async () => {
    const { cms, llmKey } = await world();
    const ws = await (await world()).workspace("qa07", { cached: true });
    const word = wordMarker("qaseven");
    const wrong = wordMarker("wrongfact");
    const question = `${word} とは何ですか`;
    const v1 = await publishAnswerable(ws, question, `${word} は ${wrong} の記事です。旧版の誤った内容を含みます。`);
    await expectCached(ws, question, "訂正の前の回答");

    // 訂正として公開する。その直後の回答は、旧版を出典にせず、旧版の内容をLLMにも渡さない
    await mockLlm.clearRequests(llmKey);
    const corrected = `${wordMarker("rightfact")} ${word} は、訂正した記事です。`;
    await cms.saveDraft(ws.editorSession, ws.id, v1.id, { body: doc(paragraph(corrected)) });
    await cms.publishEntry(ws.editorSession, ws.id, v1.id, { correction: true, note: marker("qa07-note") });
    for (const r of [await ask(ws, question), (await askStream(ws, question)).final]) {
      expect(r.cached, "訂正した記事の旧版を引用した回答キャッシュが返った").not.toBe(true);
      expect(r.sources.map((s) => s.versionId), "訂正した記事の旧版が出典に含まれる").not.toContain(v1.versionId);
      expect(r.text, "訂正した記事の旧版の内容が回答に含まれる").not.toContain(wrong);
    }
    const sent = JSON.stringify(await mockLlm.requests(llmKey));
    expect(sent.includes(wrong), "訂正した記事の旧版の内容が、LLMへのリクエストに含まれる").toBe(false);

    // 対照: 訂正した版で答えられるようになる
    const v2 = await waitAnswerable(ws, question, v1.id, v1.versionId);
    expect(v2.versionId).not.toBe(v1.versionId);
  },
  300_000,
);

inv(
  "inv_qa_08",
  "回答キャッシュに質問文の平文は保存されない",
  async () => {
    const w = await world();
    const ws = await w.workspace("qa08", { cached: true });
    const word = wordMarker("qaeight");
    const article = `${word} は、質問文の保存を確かめるための記事です。`;
    // 質問にだけ含まれる目印（記事には書かない）
    const secret = wordMarker("questiononly");
    const question = `${word} と ${secret} について教えてください`;
    await publishAnswerable(ws, question, article);
    // キャッシュに載ったことを確かめる（載っていなければ、この約束を確かめたことにならない）
    await expectCached(ws, question, "質問の目印を含む質問");

    // DBのどこにも、質問文の目印がない（大文字・小文字の違いも含めて）
    expect(await driver.storage.findText(w.config, secret), "質問文の平文がDBに保存されている").toEqual([]);
    // 対照: 記事の本文の目印は見つかる（DBを探せている）
    expect((await driver.storage.findText(w.config, word)).length, "記事の目印がDBに見つからない（探し方が間違っている）").toBeGreaterThan(0);
  },
  300_000,
);

inv(
  "inv_qa_09",
  "確認期限切れの判定はキャッシュから返すときにも現在時刻で行われ、確認期限をまたいだキャッシュは返されない",
  async () => {
    const w = await world();
    const ws = await w.workspace("qa09", { cached: true });
    const word = wordMarker("qanine");
    const question = `${word} とは何ですか`;
    const { id } = await publishAnswerable(ws, question, `${word} は、確認期限を確かめるための記事です。`);
    // 確認期限（テンプレートの既定は1日）が、少し先に来るようにする
    const dueInSeconds = 40;
    await driver.storage.setLastVerifiedAgo(w.config, ws.id, id, 24 * 60 * 60 - dueInSeconds);
    const deadline = Date.now() + dueInSeconds * 1000;

    // 期限の前: 作り直した回答は期限切れでない。同じ質問はキャッシュから返る
    await mockLlm.clearRequests(w.llmKey);
    const before = await ask(ws, question);
    expect(before.status).toBe("answered");
    expect(before.sources.find((s) => s.entryId === id)?.reviewExpired, "期限の前なのに、期限切れになっている").toBe(false);
    const hit = await expectCached(ws, question, "期限の前の同じ質問");
    expect(hit.sources.find((s) => s.entryId === id)?.reviewExpired).toBe(false);
    expect(Date.now() < deadline, "期限の前に確かめ終わらない（期限までの時間を長くする）").toBe(true);

    // 期限をまたいだ: キャッシュは返されず、出典は今の時刻で期限切れと判定される
    await sleep((deadline - Date.now()) / 1000 + 3);
    for (const r of [await ask(ws, question), (await askStream(ws, question)).final]) {
      expect(r.cached, "確認期限をまたいだ回答キャッシュが返った").not.toBe(true);
      const source = r.sources.find((s) => s.entryId === id);
      expect(source, "期限切れの記事も根拠に使われるはず").toBeDefined();
      expect(source!.reviewExpired, "期限を過ぎたのに、出典が期限切れと判定されない").toBe(true);
    }
    // 期限切れのまま作り直した回答は、キャッシュから返るときも期限切れと判定される
    const expiredHit = await expectCached(ws, question, "期限切れの後の同じ質問");
    expect(expiredHit.sources.find((s) => s.entryId === id)?.reviewExpired).toBe(true);
  },
  300_000,
);
