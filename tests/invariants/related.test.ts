// 関連記事の約束: INV-REL-01（M3、08-search-and-related.md「関連記事」、Q61）
//
// - 関連記事は、記事全体のベクトル（チャンクの埋め込みから計算）で、公開のときと毎日の一括計算のときに計算して保存する。
//   返す時点で公開状態などを照合するので、一覧を保存した後に非公開・削除された記事も、次の一括計算の前から出ない。
// - 舞台は、本文を検索の対象にしたテンプレートのワークスペース（support/world.ts の lazySearchWorld）。テスト環境のCMSは
//   擬似LLMで埋め込みを作る。関連記事は同じコンテンツタイプ・同じロケールの記事から選ぶ。
// - 編集者が固定した記事も、非公開・削除されたら出ない（固定は翻訳グループを指し、返すときに照合する）。

import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asSession, type RegisteredQuery, type RelatedEntry, type SearchResult } from "./support/driver.js";
import { expectSuccess } from "./support/assert.js";
import { simpleBody } from "./support/body.js";
import { lazySearchWorld, marker, sleep, type SearchWorkspace } from "./support/world.js";

const world = lazySearchWorld();

/** 本文の共通の部分（関連記事に選ばれるように、似た内容にする）。 */
const TEXT = "関連記事の約束を確かめるための記事です。";
/**
 * 記事ごとの本文。共通の部分に、記事ごとに違う文を足す。ほぼ同じ内容（類似度 0.97 以上）の記事どうしは、
 * 新しいほうだけが残る決まりなので、記事ごとに内容を変える（擬似LLMの埋め込みで、どの組も 0.85 未満）。
 */
const BODIES: Record<string, string> = {
  A: "春の庭で育てるトマトとバジルの話をします。",
  B: "夏の畑で育てるキュウリとナスの話をします。",
  C: "秋の庭で育てるカボチャとサツマイモの話をします。",
  D: "冬の温室で育てるイチゴとレタスの話をします。",
  E: "ベランダで育てるハーブとミニトマトの話をします。",
};

async function publish(ws: SearchWorkspace, label: string): Promise<string> {
  const { cms } = await world();
  const { id } = await cms.createEntry(ws.editorSession, ws.id, { title: marker(`rel-${label}`), body: simpleBody(`${TEXT}${BODIES[label]}`) });
  await cms.publishEntry(ws.editorSession, ws.id, id);
  return id;
}

/** 記事が検索に出るようになる（埋め込みができる）まで待つ。 */
async function waitSearchable(ws: SearchWorkspace, ids: string[]): Promise<void> {
  const { cms } = await world();
  const deadline = Date.now() + 120_000;
  for (;;) {
    const r: SearchResult = await cms.search(ws.id, ws.readKey.secret, ws.searchQuery, { text: TEXT, first: 50 });
    const found = r.hits.map((h) => h.entryId);
    if (ids.every((id) => found.includes(id))) return;
    if (Date.now() > deadline) throw new Error(`公開した記事が検索に出ない: ${ids.filter((id) => !found.includes(id)).join(", ")}`);
    await sleep(1);
  }
}

inv(
  "inv_rel_01",
  "関連記事には、非公開・削除された記事が、次の一括再計算の前でも含まれない",
  async () => {
    const { cms, a } = await world();
    const query: RegisteredQuery = await cms.registerQuery(a.id, a.registerKey.secret, { kind: "relatedEntries" });
    const related = (id: string): Promise<RelatedEntry[]> => cms.relatedEntries(a.id, a.readKey.secret, query, { id, first: 10 });

    // 1. 似た記事を先に公開し、埋め込みができるまで待つ
    const b = await publish(a, "B");
    const c = await publish(a, "C");
    const d = await publish(a, "D");
    const e = await publish(a, "E");
    await waitSearchable(a, [b, c, d, e]);
    // 2. A を公開する。A の関連記事が計算される（公開のとき）まで待つ
    const target = await publish(a, "A");
    const deadline = Date.now() + 120_000;
    for (;;) {
      const ids = (await related(target)).map((r) => r.id);
      if ([b, c, e].every((id) => ids.includes(id))) break;
      if (Date.now() > deadline) throw new Error(`関連記事が計算されない（今の一覧: ${ids.join(", ")}）`);
      await sleep(1);
    }
    expect((await related(target)).map((r) => r.id), "関連記事に記事そのものが含まれる").not.toContain(target);

    // 3. 対照: D を固定すると、先頭に出る
    await cms.reauthenticate(a.adminSession, a.admin);
    const groupD = await cms.translationGroupId(a.editorSession, a.id, d);
    expectSuccess(await cms.setRelatedRaw(asSession(a.adminSession), a.id, target, { pinned: [groupD], excluded: [] }), "関連記事の固定");
    const pinned = await related(target);
    expect(pinned[0]?.id, "固定した記事が先頭に出ない").toBe(d);
    expect(pinned[0]?.pinned, "固定した記事に、固定の印がない").toBe(true);

    // 4. B を非公開にし、C を削除し、固定した D を非公開にする
    await cms.unpublishEntry(a.editorSession, a.id, b);
    await cms.deleteEntry(a.editorSession, a.id, c);
    await cms.unpublishEntry(a.editorSession, a.id, d);

    // 5. すぐに（一括計算の前に）見ても、少し間をおいて見ても出ない
    for (const wait of [0, 3]) {
      await sleep(wait);
      const ids = (await related(target)).map((r) => r.id);
      expect(ids, "非公開にした記事が関連記事に含まれる").not.toContain(b);
      expect(ids, "削除した記事が関連記事に含まれる").not.toContain(c);
      expect(ids, "固定した記事が、非公開にした後も関連記事に含まれる").not.toContain(d);
      // 対照: 公開のままの記事は出る
      expect(ids, "公開のままの記事が関連記事に出ない").toContain(e);
    }
  },
  360_000,
);
