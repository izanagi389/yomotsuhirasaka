// 編集者向けのAI支援の予算の約束: INV-BUD-02（M4、10-llm-and-budget.md「予算」「編集者向けのAI支援」、Q70）
//
// - 支援（矛盾の検知）は、ワークスペースの「編集者向けの支援」の予算の範囲でだけ LLM を呼ぶ。尽きたら、支援の部分に
//   「今月の上限に達しました」などと出すだけで、記事の公開は妨げない。
// - 矛盾の検知は、公開の確認画面が始める非同期の処理。公開そのものは、支援の結果や状態を見ない。
// - 舞台は専用のインスタンス（support/world.ts の createAnswerWorld）と専用の擬似LLMのキー。擬似LLMの記録はキーごとなので、
//   チャットの要求が届いたかどうかを、他のテストと混ざらずに確かめられる。
// - 公開が「妨げられない」ことは、公開の応答が成功し、その記事が配信API（検索）に出ることで確かめる。
// - 対照: 予算が残っていれば、矛盾の検知は LLM を呼んで終わる。
// - 検索語はマスク（10桁以上の数字の並び → [number]。Q169）で変わらないよう、数字を含めない。

import { afterAll, expect } from "vitest";
import { inv } from "./support/inv.js";
import { asSession, type ConflictCheck } from "./support/driver.js";
import { expectSuccess } from "./support/assert.js";
import { doc, paragraph } from "./support/body.js";
import { chatRequests, mockLlm, noConflictOutput } from "./support/mock-llm.js";
import { createAnswerWorld, marker, sleep, type AnswerWorkspace, type AnswerWorld } from "./support/world.js";

let worldPromise: Promise<AnswerWorld> | null = null;
const world = (): Promise<AnswerWorld> => (worldPromise ??= createAnswerWorld());

afterAll(async () => {
  if (worldPromise) await (await worldPromise).stop();
});

/** 支援の予算の、十分に大きい天井。 */
const LARGE = 10_000_000;

/** 数字を含まない一意な目印（マスクで変わらないようにする）。 */
const wordMarker = (label: string): string => marker(label).replace(/\d/g, (d) => "ghijklmnop"[Number(d)]!);

/** 記事が配信API（検索）に出るまで待つ。時間内に出なければ失敗する。 */
async function waitSearchable(ws: AnswerWorkspace, entryId: string, word: string, what: string): Promise<void> {
  const { cms } = await world();
  const deadline = Date.now() + 120_000;
  for (;;) {
    const r = await cms.search(ws.id, ws.readKey.secret, ws.searchQuery, { text: word, first: 50 });
    if (r.hits.some((h) => h.entryId === entryId)) return;
    expect(Date.now() < deadline, `${what}: 公開した記事が配信APIに出ない`).toBe(true);
    await sleep(1);
  }
}

/** 矛盾の検知が running でなくなるまで待つ。 */
async function settled(ws: AnswerWorkspace, entryId: string): Promise<ConflictCheck> {
  const { cms } = await world();
  const deadline = Date.now() + 180_000;
  for (;;) {
    const r = await cms.conflictCheck(ws.editorSession, ws.id, entryId);
    if (r.state !== "running") return r;
    expect(Date.now() < deadline, "矛盾の検知が終わらない").toBe(true);
    await sleep(1);
  }
}

/** 比べる相手の記事と言葉を共有する下書き（まだ公開しない）。own はこの下書きだけの検索語。 */
async function draftAbout(ws: AnswerWorkspace, topic: string, own: string): Promise<string> {
  const { cms } = await world();
  const text = `${topic} の料金と提供時間について説明します。${own} の手順もあわせて案内します。`;
  return (await cms.createEntry(ws.editorSession, ws.id, { title: marker("bud02-draft"), body: doc(paragraph(text)) })).id;
}

/** 編集者が公開する。公開の応答が成功し、その記事が配信APIに出ることを確かめる。 */
async function expectPublished(ws: AnswerWorkspace, entryId: string, own: string, what: string): Promise<void> {
  const { cms } = await world();
  expectSuccess(await cms.publishRaw(asSession(ws.editorSession), ws.id, entryId), `${what}: 公開`);
  await waitSearchable(ws, entryId, own, what);
}

async function setBudget(ws: AnswerWorkspace, month: number | null, day: number | null): Promise<void> {
  const { cms } = await world();
  await cms.reauthenticate(ws.adminSession, ws.admin);
  const pair = { tokensPerMonth: month, tokensPerDay: day };
  expectSuccess(await cms.setAssistBudgetRaw(asSession(ws.adminSession), ws.id, { input: pair, output: pair }), "管理者による支援の予算の設定");
}

inv(
  "inv_bud_02",
  "編集者向けの支援の予算が尽きても、記事の公開は妨げられない",
  async () => {
    const w = await world();
    const { cms, llmKey } = w;
    const ws = await w.workspace("bud02", { assistCeiling: LARGE });
    await cms.reauthenticate(ws.adminSession, ws.admin);
    await cms.setAssistSettings(ws.adminSession, ws.id, { enabled: true });

    // 比べる相手: 意味の近い、公開中の別の記事
    const topic = wordMarker("bud02-topic");
    const other = await cms.createEntry(ws.editorSession, ws.id, {
      title: marker("bud02-other"),
      body: doc(paragraph(`${topic} の料金は月ごとに決まり、提供時間は平日の朝から夕方までです。`)),
    });
    await cms.publishEntry(ws.editorSession, ws.id, other.id);
    await waitSearchable(ws, other.id, topic, "比べる相手の記事");
    await mockLlm.setStrategy(llmKey, { kind: "fixed", content: noConflictOutput() });

    /** 下書きを作り、矛盾の検知を始めて終わるまで待つ。その間に届いたチャットの要求の数も返す。 */
    const checkDraft = async (label: string): Promise<{ id: string; own: string; result: ConflictCheck; chats: number }> => {
      await mockLlm.clearRequests(llmKey);
      const own = wordMarker(`bud02-${label}`);
      const id = await draftAbout(ws, topic, own);
      await cms.startConflictCheck(ws.editorSession, ws.id, id);
      const result = await settled(ws, id);
      const chats = chatRequests(await mockLlm.waitUntilQuiet(llmKey, 3_000)).length;
      return { id, own, result, chats };
    };

    // --- 対照: 予算が残っていれば、矛盾の検知は LLM を呼んで終わる ---
    const control = await checkDraft("control");
    expect(control.result.state, "予算が残っているのに、矛盾の検知が終わらない").toBe("done");
    expect(control.chats, "予算が残っているのに、矛盾の検知で LLM が呼ばれない").toBeGreaterThan(0);
    await expectPublished(ws, control.id, control.own, "予算が残っているとき");

    // --- 1. 月の予算が尽きた: 支援は止まり（LLM を呼ばない）、公開はできる ---
    await setBudget(ws, 1, null);
    const month = await checkDraft("month");
    expect(month.result.state, "月の予算が尽きたのに、支援の状態が予算切れでない").toBe("budgetExhausted");
    expect(month.result.period, "予算切れの期間が月でない").toBe("month");
    expect(month.chats, "月の予算が尽きたのに、LLM が呼ばれた").toBe(0);
    await expectPublished(ws, month.id, month.own, "月の予算が尽きたとき");

    // --- 2. 日の予算が尽きた ---
    await setBudget(ws, null, 1);
    const day = await checkDraft("day");
    expect(day.result.state, "日の予算が尽きたのに、支援の状態が予算切れでない").toBe("budgetExhausted");
    expect(day.result.period, "予算切れの期間が日でない").toBe("day");
    expect(day.chats, "日の予算が尽きたのに、LLM が呼ばれた").toBe(0);
    await expectPublished(ws, day.id, day.own, "日の予算が尽きたとき");

    // --- 3. サーバー管理者の天井が 0（ワークスペースの上限は決めていない） ---
    await setBudget(ws, null, null);
    const sa = await cms.login(w.serverAdmin);
    await cms.reauthenticate(sa, w.serverAdmin);
    await cms.setWorkspaceLimits(sa, ws.id, {
      assistInputTokensPerMonth: 0,
      assistInputTokensPerDay: 0,
      assistOutputTokensPerMonth: 0,
      assistOutputTokensPerDay: 0,
    });
    const zero = await checkDraft("zero");
    expect(zero.result.state, "天井が 0 なのに、支援の状態が予算切れでない").toBe("budgetExhausted");
    expect(zero.chats, "天井が 0 なのに、LLM が呼ばれた").toBe(0);
    await expectPublished(ws, zero.id, zero.own, "天井が 0 のとき");
    await cms.setWorkspaceLimits(sa, ws.id, {
      assistInputTokensPerMonth: LARGE,
      assistInputTokensPerDay: LARGE,
      assistOutputTokensPerMonth: LARGE,
      assistOutputTokensPerDay: LARGE,
    });

    // --- 4. 支援が動いている途中（LLM が応答しない）でも、公開は待たされず、支援の結果を待たない ---
    await mockLlm.clearRequests(llmKey);
    await mockLlm.setFaults(llmKey, [{ kind: "timeout", endpoint: "chat", delayMs: 120_000 }]);
    try {
      const own = wordMarker("bud02-running");
      const id = await draftAbout(ws, topic, own);
      await cms.startConflictCheck(ws.editorSession, ws.id, id);
      await mockLlm.waitForRequests(llmKey, (r) => chatRequests(r).length > 0);
      expect((await cms.conflictCheck(ws.editorSession, ws.id, id)).state, "LLM が応答していないのに、矛盾の検知が終わった").toBe("running");
      const started = Date.now();
      expectSuccess(await cms.publishRaw(asSession(ws.editorSession), ws.id, id), "支援が動いている途中の公開");
      expect(Date.now() - started, "公開が、支援の LLM の応答を待たされた").toBeLessThan(15_000);
      await waitSearchable(ws, id, own, "支援が動いている途中");

      // LLM の障害（タイムアウト）で支援が終わらなくても、公開済みの記事はそのまま
      const failed = await settled(ws, id);
      expect(failed.state, "LLM がタイムアウトしたのに、支援の状態が失敗でない").toBe("failed");
      await waitSearchable(ws, id, own, "支援が LLM の障害で失敗した後");
    } finally {
      await mockLlm.clearFaults(llmKey);
    }
  },
  900_000,
);
