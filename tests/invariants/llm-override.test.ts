// LLMの接続設定の上書きの約束: INV-LLM-01・02（M4、10-llm-and-budget.md「接続設定の単位」、Q26、13-security.md）
//
// - 舞台は専用のインスタンス（support/world.ts の createAnswerWorld）。サーバー管理者が許可する接続先は、擬似LLMの1つだけ
//   （名前は "mock"）。ワークスペースは、その名前で選び、自分のAPIキーとモデル名を設定する。
// - 擬似LLMは、APIキーごとに記録する。ワークスペースが設定したキーで擬似LLMに依頼が届けば、上書きが効いている。

import { afterAll, afterEach, expect } from "vitest";
import { inv } from "./support/inv.js";
import { asSession, type AnswerResult, type OperationTarget } from "./support/driver.js";
import type { HttpResponse } from "./support/http.js";
import { doc, paragraph } from "./support/body.js";
import { chatRequests, mockLlm } from "./support/mock-llm.js";
import { createAnswerWorld, marker, sleep, type AnswerWorkspace, type AnswerWorld } from "./support/world.js";

/** サーバー管理者が許可する接続先の名前（擬似LLM）。 */
const ALLOWED = "mock";
const mockUrl = mockLlm.connection("x").baseUrl;

let worldPromise: Promise<AnswerWorld> | null = null;
const world = (): Promise<AnswerWorld> =>
  (worldPromise ??= createAnswerWorld({ llm: { allowedEndpoints: [{ name: ALLOWED, baseUrl: mockUrl }] } }));

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
  const { cms } = await world();
  const { id } = await cms.createEntry(ws.editorSession, ws.id, { title: marker("llm-title"), body: doc(paragraph(text)) });
  await cms.publishEntry(ws.editorSession, ws.id, id);
  const deadline = Date.now() + 120_000;
  for (;;) {
    const r = await cms.search(ws.id, ws.readKey.secret, ws.searchQuery, { text: word, first: 50 });
    if (r.hits.some((h) => h.entryId === id)) return id;
    if (Date.now() > deadline) throw new Error(`公開した記事が検索に出ない（埋め込みができない）: ${id}`);
    await sleep(1);
  }
}

async function ask(ws: AnswerWorkspace, question: string): Promise<AnswerResult> {
  const { cms } = await world();
  const res = await cms.answerRaw(ws.id, ws.answerKey.secret, ws.answerQuery, { question });
  const r = cms.parseAnswer(res);
  expect(r, `回答が返らない（${res.status}）: ${JSON.stringify(cms.answerErrors(res))}`).not.toBeNull();
  return r!;
}

/** LLMの接続設定の上書きを送る（管理者と再認証）。 */
async function setLlm(ws: AnswerWorkspace, body: Record<string, unknown>): Promise<HttpResponse> {
  const { cms } = await world();
  await cms.reauthenticate(ws.adminSession, ws.admin);
  return cms.setWorkspaceLlmRaw(asSession(ws.adminSession), ws.id, body);
}

const isRejected = (res: HttpResponse) => res.status >= 400 && res.status < 500;

inv(
  "inv_llm_01",
  "ワークスペースは、サーバー管理者が許可していないLLMのエンドポイントを設定できない",
  async () => {
    const w = await world();
    const ws = await w.workspace("llm01", { llmOverride: true });
    const denied = await w.workspace("llm01-denied");
    const word = wordMarker("llmone");
    await publishSearchable(ws, word, `${word} は、LLMの接続設定の上書きを確かめるための記事です。`);
    const key = `inv-llm01-${wordMarker("key").toLowerCase()}`;
    const valid = { endpoint: ALLOWED, apiKey: key, chatModel: mockLlm.chatModel };

    // 許可していない接続先は選べない（一覧にない名前・URLを名前として渡す・URLを別の項目で直接渡す）
    for (const [body, what] of [
      [{ ...valid, endpoint: "not-allowed" }, "許可の一覧にない名前"],
      [{ ...valid, endpoint: mockUrl }, "URLを名前として渡したもの"],
      [{ ...valid, endpoint: "http://169.254.169.254/latest" }, "内部の宛先のURL"],
      [{ ...valid, baseUrl: "http://169.254.169.254/latest" }, "URLを別の項目で直接渡したもの"],
    ] as const) {
      const res = await setLlm(ws, body);
      expect(isRejected(res), `${what}: 許可していない接続先が設定できた（${res.status}）: ${res.text}`).toBe(true);
    }
    // サーバー管理者が上書きを許していないワークスペースは、許可した接続先でも設定できない
    const res = await setLlm(denied, valid);
    expect(isRejected(res), `上書きを許していないワークスペースで設定できた（${res.status}）: ${res.text}`).toBe(true);

    // 対照: 許可した接続先は設定でき、回答はその接続先とキーで作られる
    const ok = await setLlm(ws, valid);
    expect(ok.status, `許可した接続先を設定できない（${ok.status}）: ${ok.text}`).toBeLessThan(300);
    await mockLlm.clearRequests(key);
    expect((await ask(ws, `${word} とは何ですか`)).status).toBe("answered");
    expect(chatRequests(await mockLlm.requests(key)).length, "上書きした接続先とキーで、回答が作られない").toBeGreaterThan(0);
  },
  300_000,
);

inv(
  "inv_llm_02",
  "保存したLLMのAPIキーは、管理画面や管理APIから再取得できない",
  async () => {
    const w = await world();
    const { cms } = w;
    const ws = await w.workspace("llm02", { llmOverride: true });
    const word = wordMarker("llmtwo");
    const entryId = await publishSearchable(ws, word, `${word} は、LLMのAPIキーの扱いを確かめるための記事です。`);
    // APIキーに目印を入れる（擬似LLMは、どんなキーでも受け付けて、キーごとに記録する）
    const secret = wordMarker("secretkey").toLowerCase();
    const key = `inv-llm02-${secret}`;
    const saved = await setLlm(ws, { endpoint: ALLOWED, apiKey: key, chatModel: mockLlm.chatModel });
    expect(saved.status, `接続設定を保存できない（${saved.status}）: ${saved.text}`).toBeLessThan(300);
    expect(saved.text, "保存の応答に、APIキーが含まれる").not.toContain(secret);

    // 管理APIの読み取りの操作（ワークスペースと本人に向けたもの）の、どの応答にもキーが含まれない
    await cms.reauthenticate(ws.adminSession, ws.admin);
    const llm = await cms.getWorkspaceLlmRaw(asSession(ws.adminSession), ws.id);
    expect(llm.status, `接続設定を読めない（${llm.status}）: ${llm.text}`).toBe(200);
    expect(llm.text, "接続設定の読み取りに、APIキーが含まれる").not.toContain(secret);
    const target: OperationTarget = { workspaceId: ws.id, entryId, userId: ws.admin.userId, apiKeyId: ws.answerKey.id };
    let read = 0;
    for (const op of cms.adminOperations.filter((o) => !o.mutating && o.scope !== "instance")) {
      await cms.reauthenticate(ws.adminSession, ws.admin);
      const res = await op.call(asSession(ws.adminSession), target);
      expect(res.text, `管理APIの「${op.name}」の応答に、保存したAPIキーが含まれる`).not.toContain(secret);
      if (res.status < 300) read++;
    }
    expect(read, "管理APIの読み取りの操作が、ほとんど成功していない（確かめたことにならない）").toBeGreaterThan(10);

    // 対照: 保存したキーは実際に使われている（擬似LLMに、そのキーで依頼が届く）
    await mockLlm.clearRequests(key);
    expect((await ask(ws, `${word} とは何ですか`)).status).toBe("answered");
    expect(chatRequests(await mockLlm.requests(key)).length, "保存したAPIキーが、回答に使われない").toBeGreaterThan(0);
  },
  300_000,
);
