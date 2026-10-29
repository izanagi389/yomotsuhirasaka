// 埋め込みモデルの切り替えの約束: INV-EMB-01（M3、08-search-and-related.md「モデルの変更」、Q30）
//
// - 新しいモデルは設定の `[llm.next_embedding]` に書き、サーバー管理者が見積もりを見てから開始する。新しい埋め込みが
//   ワークスペースのすべてのチャンクにそろうまでは、検索は旧モデルで行い、質問も旧モデルで埋め込む。
// - 擬似LLMの記録と障害はAPIキーごとなので、専用のインスタンスを起動し、旧モデルと新しいモデルに別のキーを使う。
//   新しいモデルのキーにだけ、埋め込みが失敗し続ける障害を入れて、切り替えを途中で止める（旧モデルの検索には影響しない）。
//   予算で止めると、質問の埋め込みも同じ予算から出るため、検索そのものが予算切れになって確かめられない（予算は INV-BUD-01）。
// - 「欠けない」ことは、切り替えの前・途中・後のどの時点でも、公開した記事がすべて検索結果に出ることで確かめる。
//   テスト環境にはキーワード検索の拡張がないので、結果はベクトル検索のものだけ。

import { randomUUID } from "node:crypto";
import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asBearer, asSession, driver, type CmsClient, type RegisteredQuery, type SearchResult, type Session } from "./support/driver.js";
import { expectSuccess } from "./support/assert.js";
import { doc, paragraph } from "./support/body.js";
import { mockLlm, type MockLlmRequest } from "./support/mock-llm.js";
import { SEARCHABLE_TEMPLATE } from "./support/template.js";
import { marker, sleep } from "./support/world.js";

const OLD_MODEL = "mock-embed-64";
const NEW_MODEL = "mock-embed-32";

/** 記事の本文（記事ごとに違う文にする）。 */
const bodyText = (word: string, i: number) => `${word} 埋め込みモデルの切り替えを確かめる記事 ${i} 番目。${randomUUID().slice(0, 8)}`;

/** 届いた埋め込みのリクエストのうち、入力が `text` だけのもの（検索語の埋め込み）。 */
function queryEmbeddings(requests: MockLlmRequest[], text: string): MockLlmRequest[] {
  return requests.filter((r) => {
    if (r.endpoint !== "embeddings" || r.status !== 200) return false;
    const input = (r.body as { input?: unknown }).input;
    const inputs = Array.isArray(input) ? input : [input];
    return inputs.length === 1 && inputs[0] === text;
  });
}

const modelOf = (r: MockLlmRequest) => (r.body as { model?: string }).model;

inv(
  "inv_emb_01",
  "埋め込みモデルの切り替え中も、切り替えの完了まで検索結果から記事が欠けない",
  async () => {
    const oldKey = `inv-emb01-old-${randomUUID().slice(0, 8)}`;
    const newKey = `inv-emb01-new-${randomUUID().slice(0, 8)}`;
    const instance = await driver.createIsolatedInstance();
    const oldOnly = { ...instance.config, llm: mockLlm.connection(oldKey) };
    let started = await driver.startServer(oldOnly);
    try {
      expect(started.outcome, `擬似LLMにつないだサーバーが起動しない:\n${started.output}`).toBe("ready");
      let cms: CmsClient = started.client!;
      let sa = await cms.login(instance.serverAdmin);
      await cms.reauthenticate(sa, instance.serverAdmin);
      const token = await cms.deployToken(sa, instance.serverAdmin);
      expectSuccess(await cms.deployTemplateRaw(asBearer(token), SEARCHABLE_TEMPLATE), "検索の対象のフィールドを持つテンプレートのデプロイ");
      const ws = (await cms.createWorkspace(sa, "inv-emb-01", { template: SEARCHABLE_TEMPLATE.name })).id;
      // 予算は十分に大きくする（予算では止めない）
      await cms.setWorkspaceLimits(sa, ws, { embeddingTokensPerMonth: 10_000_000, embeddingTokensPerDay: 10_000_000 });
      const admin = await cms.createMember(sa, ws, "admin");
      let session: Session = await cms.login(admin);
      await cms.reauthenticate(session, admin);
      expectSuccess(await cms.setEmbeddingBudgetRaw(asSession(session), ws, { tokensPerMonth: null, tokensPerDay: null }), "埋め込みの予算の設定");
      const readKey = await cms.createApiKey(session, ws, ["read"]);
      const registerKey = await cms.createApiKey(session, ws, ["register"]);
      const query: RegisteredQuery = await cms.registerQuery(ws, registerKey.secret, { kind: "search" });

      // 検索語は外部に送る前にマスクされ、10桁以上の数字の並び（`-` で区切られていてもよい）は `[number]` になる（Q169）。
      // 擬似LLMに届いた入力と完全に一致させて比べるため、検索語には数字を含めない（UUIDの数字を英字に置き換える）
      const word = marker("emb01").replace(/\d/g, (d) => "ghijklmnop"[Number(d)]!);
      const search = (): Promise<SearchResult> => cms.search(ws, readKey.secret, query, { text: word, first: 50 });
      const publish = async (i: number): Promise<string> => {
        const { id } = await cms.createEntry(session, ws, { title: marker("emb01-title"), body: doc(paragraph(bodyText(word, i))) });
        await cms.publishEntry(session, ws, id);
        return id;
      };
      /** すべての記事が検索結果に出ることを確かめる（ベクトル検索が使えることも）。 */
      const expectAll = async (ids: string[], when: string): Promise<void> => {
        const r = await search();
        expect(r.vectorSearch, `${when}: ベクトル検索が使えない`).toBe("available");
        const found = r.hits.map((h) => h.entryId);
        expect(ids.filter((id) => !found.includes(id)), `${when}: 検索結果から記事が欠けた`).toEqual([]);
      };
      /** すべての記事が検索に出るようになるまで待つ。 */
      const waitAll = async (ids: string[]): Promise<void> => {
        const deadline = Date.now() + 120_000;
        for (;;) {
          const found = (await search()).hits.map((h) => h.entryId);
          if (ids.every((id) => found.includes(id))) return;
          if (Date.now() > deadline) throw new Error(`公開した記事が検索に出ない: ${ids.filter((id) => !found.includes(id)).join(", ")}`);
          await sleep(1);
        }
      };

      // --- 1. 旧モデルで、5件を検索に出す ---
      const entries: string[] = [];
      for (let i = 1; i <= 5; i++) entries.push(await publish(i));
      await waitAll(entries);

      // --- 2. 新しいモデルを設定に足して起動し直す（新しいモデルは別のキー。そのキーの埋め込みは失敗し続ける） ---
      await mockLlm.setFaults(newKey, [{ kind: "serverError", endpoint: "embeddings" }]);
      await started.stop();
      started = await driver.startServer({
        ...instance.config,
        llm: { ...mockLlm.connection(oldKey), nextEmbedding: { model: NEW_MODEL, dimensions: 32, baseUrl: mockLlm.baseUrl, apiKey: newKey } },
      });
      expect(started.outcome, `新しいモデルを足したサーバーが起動しない:\n${started.output}`).toBe("ready");
      cms = started.client!;
      sa = await cms.login(instance.serverAdmin);
      session = await cms.login(admin);
      // 開始の前: 見積もりが出て、まだ始まっていない。検索は旧モデルのまま
      const before = await cms.embeddingSwitch(sa);
      expect(before.nextModel).toBe(NEW_MODEL);
      const mine = before.workspaces.find((w) => w.workspaceId === ws);
      expect(mine?.state, "開始の前に、切り替えが始まっている").toBe("notStarted");
      expect(mine?.chunksToEmbed, "新しく作るチャンクの数が見積もられていない").toBeGreaterThanOrEqual(entries.length);
      await expectAll(entries, "新しいモデルを設定に足した後");

      // --- 3. 開始する。新しいモデルの埋め込みは作れないので、途中で止まる ---
      await cms.reauthenticate(sa, instance.serverAdmin);
      expectSuccess(await cms.startEmbeddingSwitchRaw(asSession(sa)), "埋め込みモデルの切り替えの開始");
      // 新しいモデルの埋め込みを作ろうとして失敗している（切り替えの途中にある）
      await mockLlm.waitForRequests(newKey, (rs) => rs.some((r) => r.endpoint === "embeddings" && r.status >= 500));
      // 途中で公開した記事も、検索に出る（旧モデルの埋め込みも作る）
      entries.push(await publish(6));
      await waitAll(entries);
      // 途中のどの時点でも欠けない。質問は旧モデルで埋め込まれている
      for (let i = 0; i < 10; i++) {
        await expectAll(entries, "切り替えの途中");
        await sleep(1);
      }
      expect(queryEmbeddings(await mockLlm.requests(newKey), word), "切り替えの完了前に、質問が新しいモデルで埋め込まれた").toEqual([]);
      expect(
        queryEmbeddings(await mockLlm.requests(oldKey), word).map(modelOf),
        "切り替えの途中に、質問が旧モデルで埋め込まれていない",
      ).toContain(OLD_MODEL);
      const during = (await cms.embeddingSwitch(sa)).workspaces.find((w) => w.workspaceId === ws);
      expect(during?.state, "埋め込みを作れないのに、切り替わった").not.toBe("switched");

      // --- 4. 障害を消して再開する。切り替わる（質問が新しいモデルで埋め込まれる）まで、ずっと欠けない ---
      await mockLlm.clearFaults(newKey);
      await cms.reauthenticate(sa, instance.serverAdmin);
      expectSuccess(await cms.startEmbeddingSwitchRaw(asSession(sa)), "埋め込みモデルの切り替えの再開");
      const deadline = Date.now() + 180_000;
      for (;;) {
        await expectAll(entries, "切り替えの完了の前後");
        if (queryEmbeddings(await mockLlm.requests(newKey), word).some((r) => modelOf(r) === NEW_MODEL)) break;
        if (Date.now() > deadline) throw new Error("新しいモデルに切り替わらない");
        await sleep(1);
      }
      // 対照: 切り替わった後も欠けず、状態も切り替わっている
      await expectAll(entries, "切り替えの後");
      const after = (await cms.embeddingSwitch(sa)).workspaces.find((w) => w.workspaceId === ws);
      expect(after?.state).toBe("switched");
    } finally {
      await started.stop();
      await mockLlm.clearFaults(newKey);
    }
  },
  600_000,
);
