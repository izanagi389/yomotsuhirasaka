// 質問ログと拒否する語の約束: INV-LOG-01・02、INV-QA-16（M4、09-qa-chatbot.md「質問ログ」「拒否する語」、13-security.md）
//
// - 舞台は専用のインスタンス（support/world.ts の createAnswerWorld）。保存期間を過ぎた質問ログを消す常駐の掃除を
//   確かめるため、掃除の間隔を短くする（loginHistoryRetention の sweepIntervalSeconds。掃除はほかの記録と同じ処理）。
// - 質問ログに残るのは、答えられなかった質問だけ（答えた質問は件数だけ）。答えられない質問は、擬似LLMに「答えられない」
//   と宣言させて作る。
// - 「保存しない」こと（LOG-01）と、保存期間（LOG-02）は、CLAUDE.md の例外として driver.storage でDBに直接つなぐ。
//   QA-16 は例外に入らないので、質問ログ（有効にしたもの）に残らないことを管理APIで確かめる。
// - 質問文は外部に送る前にマスクされるので、目印には数字を含めない（wordMarker）。

import { afterAll, afterEach, expect } from "vitest";
import { inv } from "./support/inv.js";
import { asSession, driver, type AnswerResult } from "./support/driver.js";
import type { HttpResponse } from "./support/http.js";
import { doc, paragraph } from "./support/body.js";
import { chatRequests, embeddingInputs, fixedAnswer, mockLlm } from "./support/mock-llm.js";
import { createAnswerWorld, marker, sleep, type AnswerWorkspace, type AnswerWorld } from "./support/world.js";

let worldPromise: Promise<AnswerWorld> | null = null;
const world = (): Promise<AnswerWorld> =>
  (worldPromise ??= createAnswerWorld({ loginHistoryRetention: { retentionSeconds: 30 * 24 * 60 * 60, sweepIntervalSeconds: 2 } }));

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
async function publishSearchable(ws: AnswerWorkspace, word: string, text: string): Promise<string> {
  const { yomi } = await world();
  const { id } = await yomi.createEntry(ws.editorSession, ws.id, { title: marker("log-title"), body: doc(paragraph(text)) });
  await yomi.publishEntry(ws.editorSession, ws.id, id);
  const deadline = Date.now() + 120_000;
  for (;;) {
    const r = await yomi.search(ws.id, ws.readKey.secret, ws.searchQuery, { text: word, first: 50 });
    if (r.hits.some((h) => h.entryId === id)) return id;
    if (Date.now() > deadline) throw new Error(`公開した記事が検索に出ない（埋め込みができない）: ${id}`);
    await sleep(1);
  }
}

async function askRaw(ws: AnswerWorkspace, question: string, stream = false): Promise<HttpResponse> {
  const { yomi } = await world();
  return yomi.answerRaw(ws.id, ws.answerKey.secret, stream ? ws.answerStreamQuery : ws.answerQuery, { question });
}

/** まとめて返す回答。エラーなら、その内容で失敗する。 */
async function ask(ws: AnswerWorkspace, question: string): Promise<AnswerResult> {
  const { yomi } = await world();
  const res = await askRaw(ws, question);
  const r = yomi.parseAnswer(res);
  expect(r, `回答が返らない（${res.status}）: ${JSON.stringify(yomi.answerErrors(res))}`).not.toBeNull();
  return r!;
}

/** 擬似LLMに「答えられない」と宣言させて、答えられない質問をする（質問ログに残る種類の質問）。 */
async function askUnanswerable(ws: AnswerWorkspace, question: string): Promise<void> {
  const { llmKey } = await world();
  await mockLlm.setStrategy(llmKey, { kind: "fixed", content: fixedAnswer([], "答えられない") });
  const r = await ask(ws, question);
  expect(r.status, "答えられない質問が「情報なし」にならない").toBe("noInformation");
  await mockLlm.resetStrategy(llmKey);
}

/** 質問ログを有効にする（管理者と再認証）。 */
async function enableLog(ws: AnswerWorkspace, retentionDays?: number): Promise<void> {
  const { yomi } = await world();
  await yomi.reauthenticate(ws.adminSession, ws.admin);
  await yomi.setQuestionLogSettings(ws.adminSession, ws.id, retentionDays === undefined ? { enabled: true } : { enabled: true, retentionDays });
}

/** 質問ログを読める人（編集者に重い権限 viewQuestionLog を付ける）のセッション。読むたびに再認証する。 */
async function logReader(ws: AnswerWorkspace): Promise<() => Promise<string[]>> {
  const { yomi } = await world();
  await yomi.reauthenticate(ws.adminSession, ws.admin);
  const res = await yomi.setMemberGrantsRaw(asSession(ws.adminSession), ws.id, ws.editor.userId, ["viewQuestionLog"]);
  expect(res.status, `質問ログの閲覧の権限を付与できない（${res.status}）: ${res.text}`).toBeLessThan(300);
  return async () => {
    await yomi.reauthenticate(ws.editorSession, ws.editor);
    return (await yomi.questionCandidates(ws.editorSession, ws.id)).map((c) => c.question);
  };
}

inv(
  "inv_log_01",
  "質問ログを有効にしていないワークスペースでは、質問文が保存されない",
  async () => {
    const w = await world();
    const off = await w.workspace("log01-off");
    const on = await w.workspace("log01-on");
    const word = wordMarker("logone");
    const text = `${word} は、質問ログを確かめるための記事です。`;
    await publishSearchable(off, word, text);
    await publishSearchable(on, word, text);
    await enableLog(on);

    // 無効のワークスペース: 答えられた質問も、答えられなかった質問も、DBのどこにも残らない
    const offAnswered = wordMarker("offanswered");
    const offUnanswered = wordMarker("offunanswered");
    expect((await ask(off, `${word} と ${offAnswered} について`)).status).toBe("answered");
    await askUnanswerable(off, `${word} と ${offUnanswered} について`);
    for (const m of [offAnswered, offUnanswered]) {
      expect(await driver.storage.findText(w.config, m), "質問ログを有効にしていないのに、質問文が保存されている").toEqual([]);
    }

    // 対照: 有効のワークスペースでは、答えられなかった質問が「書くべき記事の候補」に出て、DBにも残る
    const onUnanswered = wordMarker("onunanswered");
    await askUnanswerable(on, `${word} と ${onUnanswered} について`);
    const read = await logReader(on);
    expect((await read()).some((q) => q.includes(onUnanswered)), "有効にした質問ログに、答えられなかった質問が出ない").toBe(true);
    expect((await driver.storage.findText(w.config, onUnanswered)).length, "有効にした質問ログが、DBに残っていない（探し方が間違っている）").toBeGreaterThan(0);
  },
  300_000,
);

inv(
  "inv_log_02",
  "質問ログを有効にした場合も、メールアドレスや電話番号などの定型パターンはマスクされて保存され、保存期間を過ぎたものは削除される",
  async () => {
    const w = await world();
    const ws = await w.workspace("log02");
    const word = wordMarker("logtwo");
    await publishSearchable(ws, word, `${word} は、質問ログのマスクと保存期間を確かめるための記事です。`);
    const retentionDays = 7;
    await enableLog(ws, retentionDays);
    const read = await logReader(ws);

    // メールアドレスと電話番号を含む、答えられない質問
    const old = wordMarker("oldquestion");
    const email = `${wordMarker("visitor").toLowerCase()}@example.com`;
    const phone = "090-4821-7356";
    await askUnanswerable(ws, `${word} ${old} について、${email} か ${phone} に連絡ください`);

    // マスクされて保存される（一覧にもDBにも元の値はない）
    const logged = (await read()).find((q) => q.includes(old));
    expect(logged, "答えられなかった質問が、質問ログに出ない").toBeDefined();
    expect(logged, "メールアドレスがマスクされずに保存された").not.toContain(email);
    expect(logged, "電話番号がマスクされずに保存された").not.toContain(phone);
    expect(logged!.includes("[email]") && logged!.includes("[number]"), "マスクした印が質問ログに出ない").toBe(true);
    for (const raw of [email, phone, phone.replace(/-/g, "")]) {
      expect(await driver.storage.findText(w.config, raw), `マスクしていない値（${raw}）がDBに保存されている`).toEqual([]);
    }
    expect((await driver.storage.findText(w.config, old)).length, "質問ログがDBに残っていない（探し方が間違っている）").toBeGreaterThan(0);

    // 保存期間を過ぎた記録は削除される（記録の時刻を保存期間より前にずらし、常駐の掃除を待つ）
    await driver.storage.setQuestionLogAgo(w.config, ws.id, (retentionDays + 1) * 24 * 60 * 60);
    // 対照: ずらした後の新しい質問は、保存期間の内側なので残る
    const fresh = wordMarker("freshquestion");
    await askUnanswerable(ws, `${word} ${fresh} について`);
    const deadline = Date.now() + 60_000;
    for (;;) {
      if ((await driver.storage.findText(w.config, old)).length === 0) break;
      if (Date.now() > deadline) throw new Error("保存期間を過ぎた質問ログが、DBから削除されない");
      await sleep(1);
    }
    const after = await read();
    expect(after.some((q) => q.includes(old)), "保存期間を過ぎた質問が、質問ログに出る").toBe(false);
    expect(after.some((q) => q.includes(fresh)), "保存期間の内側の質問まで消えた").toBe(true);
  },
  300_000,
);

inv(
  "inv_qa_16",
  "ワークスペースが拒否する語に指定した語を含む質問は、回答されず、外部のLLMにも埋め込みの作成にも送られず、質問ログにも質問文が残らない",
  async () => {
    const w = await world();
    const { yomi, llmKey } = w;
    const ws = await w.workspace("qa16");
    const word = wordMarker("qasixteen");
    await publishSearchable(ws, word, `${word} は、拒否する語を確かめるための記事です。`);
    await enableLog(ws);
    const read = await logReader(ws);
    const refused = wordMarker("refusedword").toLowerCase();
    await yomi.reauthenticate(ws.adminSession, ws.admin);
    await yomi.setAnswerSettings(ws.adminSession, ws.id, { refusal: { words: [refused], creditCard: true } });

    /** 拒否されて、LLMにも埋め込みにも何も送られない。 */
    const expectRefused = async (question: string, what: string) => {
      await mockLlm.clearRequests(llmKey);
      for (const stream of [false, true]) {
        const res = await askRaw(ws, question, stream);
        const codes = yomi.answerErrors(res).map((e) => e.code);
        expect(codes, `${what}: 拒否のエラーコードが返らない（${stream ? "ストリーミング" : "まとめて"}）`).toContain("QUESTION_REFUSED");
        expect(res.text, `${what}: 拒否したのに記事の内容が返った`).not.toContain("拒否する語を確かめる");
      }
      const sent = await mockLlm.requests(llmKey);
      expect(chatRequests(sent), `${what}: 拒否した質問がLLMに送られた`).toEqual([]);
      expect(embeddingInputs(sent), `${what}: 拒否した質問の埋め込みが作られた`).toEqual([]);
    };

    // 拒否する語（そのまま・全角と大文字に変えたもの）と、組み込みの種類（クレジットカード番号らしい並び）
    const qMarker = wordMarker("refusedquestion");
    const fullwidth = [...refused.toUpperCase()].map((c) => (c >= "!" && c <= "~" ? String.fromCharCode(c.charCodeAt(0) + 0xfee0) : c)).join("");
    await expectRefused(`${word} ${qMarker} の ${refused} について`, "拒否する語");
    await expectRefused(`${word} ${qMarker} の ${fullwidth} について`, "全角と大文字に変えた拒否する語");
    await expectRefused(`${word} ${qMarker} のカード 4111 1111 1111 1111 について`, "クレジットカード番号らしい並び");

    // 質問ログ（有効にしてある）にも、拒否した質問の文は残らない
    expect((await read()).some((q) => q.includes(qMarker)), "拒否した質問が、質問ログに出る").toBe(false);

    // 対照: 拒否する語を含まない質問は、回答される（LLMも呼ばれる）
    await mockLlm.clearRequests(llmKey);
    expect((await ask(ws, `${word} とは何ですか`)).status).toBe("answered");
    expect(chatRequests(await mockLlm.requests(llmKey)).length, "拒否する語を含まない質問で、LLMが呼ばれない").toBeGreaterThan(0);
  },
  300_000,
);
