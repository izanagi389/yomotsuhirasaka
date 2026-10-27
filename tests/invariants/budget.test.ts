// 予算の約束: INV-BUD-01
//
// 埋め込みの作成は、ワークスペースの「埋め込みの作成」の予算（トークン数、UTCの暦の月・日）の範囲でだけ行う
// （10-llm-and-budget.md、Q70）。サーバー管理者が天井を決め、ワークスペースはその範囲で上限を決める。
// - 擬似LLMの記録はAPIキーごとなので、専用のキーで、他のテストと共有しない新しいインスタンスを起動する。
// - 擬似LLMに実際に届いた入力が、予算を超えないことを確かめる。使用量（usage）を返す場合は擬似LLMの数え方で、
//   返さない場合は文字数で数える（CMSは使用量がなければ文字数から多めに数える。Q138）。
// - 公開は同時に行い、同時に呼んでも超えないことを確かめる。公開する量は、予算より必ず多くする。
// - 対照: 予算の範囲では呼ばれること、上限を上げれば止まっていた分が再開すること。

import { randomUUID } from "node:crypto";
import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asBearer, asSession, driver, type CmsClient, type EmbeddingBudget, type Session } from "./support/driver.js";
import { expectSuccess } from "./support/assert.js";
import { doc, paragraph } from "./support/body.js";
import { characterCount, embeddingInputs, mockLlm, mockUsageTokens } from "./support/mock-llm.js";
import { SEARCHABLE_TEMPLATE } from "./support/template.js";
import { marker } from "./support/world.js";

/** 予算（トークン数）。1回の公開の量より大きく、まとめて公開する量よりずっと小さくする。 */
const LIMIT = 300;
/** まとめて公開する記事の数と、1件の本文の文字数（12件 × 120文字は、どの数え方でも LIMIT を超える）。 */
const BURST = 12;
const BODY_CHARS = 120;

/** 指定した文字数の本文（記事ごとに違う文にする）。 */
function filler(chars: number): string {
  const unit = `予算を確かめる本文 ${randomUUID().slice(0, 8)}。`;
  return unit.repeat(Math.ceil(chars / unit.length)).slice(0, chars);
}

async function publish(cms: CmsClient, session: Session, workspaceId: string, chars: number): Promise<void> {
  const { id } = await cms.createEntry(session, workspaceId, { title: marker("bud01"), body: doc(paragraph(filler(chars))) });
  await cms.publishEntry(session, workspaceId, id);
}

inv(
  "inv_bud_01",
  "埋め込みの作成の予算を超えて、埋め込みAPIが呼ばれない",
  async () => {
    const apiKey = `inv-bud01-${randomUUID().slice(0, 8)}`;
    const instance = await driver.createIsolatedInstance();
    const started = await driver.startServer({ ...instance.config, llm: mockLlm.connection(apiKey) });
    try {
      expect(started.outcome, `擬似LLMにつないだサーバーが起動しない:\n${started.output}`).toBe("ready");
      const cms = started.client!;
      const sa = await cms.login(instance.serverAdmin);
      await cms.reauthenticate(sa, instance.serverAdmin);
      const token = await cms.deployToken(sa, instance.serverAdmin);
      expectSuccess(await cms.deployTemplateRaw(asBearer(token), SEARCHABLE_TEMPLATE), "検索の対象のフィールドを持つテンプレートのデプロイ");

      /** 予算を決めたワークスペースと、その管理者のセッション。天井は十分に大きくし、ワークスペースの上限で止まることを確かめる。 */
      const workspaceWithBudget = async (name: string, budget: EmbeddingBudget) => {
        const ws = (await cms.createWorkspace(sa, name, { template: SEARCHABLE_TEMPLATE.name })).id;
        await cms.setWorkspaceLimits(sa, ws, { embeddingTokensPerMonth: 1_000_000, embeddingTokensPerDay: 1_000_000 });
        const admin = await cms.createMember(sa, ws, "admin");
        const session = await cms.login(admin);
        await cms.reauthenticate(session, admin);
        expectSuccess(await cms.setEmbeddingBudgetRaw(asSession(session), ws, budget), "管理者による埋め込みの予算の設定");
        return { ws, session };
      };

      // --- 1. 使用量を返す場合（月の上限）: 擬似LLMの数え方で、届いた入力が予算を超えない ---
      const first = await workspaceWithBudget("inv-bud-01-month", { tokensPerMonth: LIMIT, tokensPerDay: null });
      await publish(cms, first.session, first.ws, 40);
      // 対照: 予算の範囲では呼ばれる
      await mockLlm.waitForRequests(apiKey, (r) => embeddingInputs(r).length > 0);
      await Promise.all(Array.from({ length: BURST }, () => publish(cms, first.session, first.ws, BODY_CHARS)));
      const withinMonth = embeddingInputs(await mockLlm.waitUntilQuiet(apiKey));
      expect(mockUsageTokens(withinMonth), "月の予算を超えて、埋め込みAPIが呼ばれた（擬似LLMの数え方）").toBeLessThanOrEqual(LIMIT);

      // 対照: 上限を上げれば、止まっていた分の埋め込みが再開する（予算で止まっていただけで、呼べなくなったのではない）
      expectSuccess(
        await cms.setEmbeddingBudgetRaw(asSession(first.session), first.ws, { tokensPerMonth: 100_000, tokensPerDay: null }),
        "管理者による埋め込みの予算の引き上げ",
      );
      await mockLlm.waitForRequests(apiKey, (r) => embeddingInputs(r).length > withinMonth.length);
      await mockLlm.waitUntilQuiet(apiKey);

      // --- 2. 使用量を返さない場合（日の上限）: CMSは文字数で数えるので、届いた入力の文字数が予算を超えない ---
      await mockLlm.clearRequests(apiKey);
      await mockLlm.setFaults(apiKey, [{ kind: "noUsage" }]);
      const second = await workspaceWithBudget("inv-bud-01-day", { tokensPerMonth: null, tokensPerDay: LIMIT });
      await publish(cms, second.session, second.ws, 40);
      await mockLlm.waitForRequests(apiKey, (r) => embeddingInputs(r).length > 0);
      await Promise.all(Array.from({ length: BURST }, () => publish(cms, second.session, second.ws, BODY_CHARS)));
      const withinDay = embeddingInputs(await mockLlm.waitUntilQuiet(apiKey));
      expect(characterCount(withinDay), "使用量を返さないとき、日の予算を超えて、埋め込みAPIが呼ばれた（文字数）").toBeLessThanOrEqual(LIMIT);
    } finally {
      await started.stop();
      await mockLlm.clearFaults(apiKey);
    }
  },
  600_000,
);
