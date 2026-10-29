// 関連記事の約束: INV-REL-01・INV-REL-02（M3、08-search-and-related.md「関連記事」、Q61）
//
// - 関連記事は、記事全体のベクトル（チャンクの埋め込みから計算）で、公開のときと毎日の一括計算のときに計算して保存する。
//   返す時点で公開状態などを照合するので、一覧を保存した後に非公開・削除された記事も、次の一括計算の前から出ない。
// - 舞台は、本文を検索の対象にしたテンプレートのワークスペース（support/world.ts の lazySearchWorld）。テスト環境のCMSは
//   擬似LLMで埋め込みを作る。関連記事は同じコンテンツタイプ・同じロケールの記事から選ぶ。
// - 編集者が固定した記事も、非公開・削除されたら出ない（固定は翻訳グループを指し、返すときに照合する）。
// - 一括計算の後と、固定・除外を変えた後に、返す一覧（返す時点の照合をかけた上限 10 件、並び順を含む）が変わった記事を、
//   Webhook（`entries.related_changed`）で知らせる。一括計算は、毎日の決まった時刻のほかに、管理APIで今すぐ動かせる。

import { randomUUID } from "node:crypto";
import { afterAll, expect } from "vitest";
import { inv } from "./support/inv.js";
import { asSession, driver, type ParsedRelatedChanged, type RegisteredQuery, type RelatedEntry, type SearchResult } from "./support/driver.js";
import { expectSuccess } from "./support/assert.js";
import { simpleBody } from "./support/body.js";
import { env } from "./support/env.js";
import { startReceiver, type Receiver, type ReceivedRequest } from "./support/net.js";
import { lazySearchWorld, marker, sleep, type SearchWorkspace } from "./support/world.js";

const world = lazySearchWorld();

let receiverPromise: Promise<Receiver> | null = null;
const receiver = () => (receiverPromise ??= startReceiver());
afterAll(async () => {
  if (receiverPromise) await (await receiverPromise).close();
});

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
  F: "春の鉢で育てるトマトとパセリの話をします。",
  G: "初夏の庭で育てるピーマンとシソの話をします。",
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

/** 届いたリクエストのうち、関連記事のイベントのもの（パスで送信先を絞る）。 */
function relatedEvents(requests: ReceivedRequest[], path: string): ParsedRelatedChanged[] {
  const type = driver.webhook.relatedChangedType();
  return requests
    .filter((r) => r.path.startsWith(path))
    .map((r) => JSON.parse(r.body) as { type?: unknown })
    .filter((v) => v.type === type)
    .map((v) => driver.webhook.parseRelatedChanged(JSON.stringify(v)));
}

inv(
  "inv_rel_02",
  "関連記事の一括再計算の後、または固定・除外の変更の後に、返す一覧が変わった記事はWebhookで知らされ、変わらなかった記事は知らされない",
  async () => {
    const { cms, a } = await world();
    const rc = await receiver();
    const path = `/rel02-${randomUUID().slice(0, 8)}`;
    await cms.reauthenticate(a.adminSession, a.admin);
    await cms.createWebhook(a.adminSession, a.id, rc.url(env.harnessAllowedHost, path));
    const query: RegisteredQuery = await cms.registerQuery(a.id, a.registerKey.secret, { kind: "relatedEntries" });
    const related = async (id: string): Promise<string[]> => (await cms.relatedEntries(a.id, a.readKey.secret, query, { id, first: 10 })).map((r) => r.id);
    const waitRelated = async (id: string, expected: string[]): Promise<void> => {
      const deadline = Date.now() + 120_000;
      for (;;) {
        const ids = await related(id);
        if (expected.every((x) => ids.includes(x))) return;
        if (Date.now() > deadline) throw new Error(`関連記事が計算されない（今の一覧: ${ids.join(", ")}）`);
        await sleep(1);
      }
    };

    // 1. B・C・D を公開してから A を公開し、A の関連記事に B・C・D が出るまで待つ
    const b = await publish(a, "B");
    const c = await publish(a, "C");
    const d = await publish(a, "D");
    await waitSearchable(a, [b, c, d]);
    const target = await publish(a, "A");
    await waitRelated(target, [b, c, d]);
    // 2. F を公開し、その後に G を公開する。G の一覧は公開のときに計算されるので、F も A も入っている
    const f = await publish(a, "F");
    await waitSearchable(a, [f]);
    const g = await publish(a, "G");
    await waitRelated(g, [target, f]);

    // 3. 一括再計算を動かし、一覧が変わった記事のイベントを集める
    const own = [target, b, c, d, f, g];
    const before = new Map<string, string[]>();
    for (const id of own) before.set(id, await related(id));
    let seen = rc.requests.length;
    expectSuccess(await cms.recomputeRelatedRaw(asSession(a.adminSession), a.id), "関連記事の一括再計算");
    await rc.waitFor((r) => relatedEvents([r], path).some((e) => e.entryIds.includes(target)), 120_000);
    // 同じ回の、残りの通が届くまで待つ
    const deadline = Date.now() + 60_000;
    for (;;) {
      const events = relatedEvents(rc.requests.slice(seen), path);
      const runs = new Set(events.map((e) => e.runId));
      const complete = [...runs].every((run) => {
        const parts = events.filter((e) => e.runId === run);
        return parts.length >= (parts[0]?.parts ?? 0);
      });
      if (complete) break;
      if (Date.now() > deadline) throw new Error("一括再計算のイベントが全部届かない");
      await sleep(1);
    }
    await sleep(3);
    const notified = new Set(relatedEvents(rc.requests.slice(seen), path).flatMap((e) => e.entryIds));
    const after = new Map<string, string[]>();
    for (const id of own) after.set(id, await related(id));
    const changed = (id: string) => JSON.stringify(before.get(id)) !== JSON.stringify(after.get(id));
    // 対照: A の一覧には F と G が入り、G の一覧は変わらない
    expect(changed(target), "対照: 一括再計算で A の一覧が変わらない").toBe(true);
    expect(after.get(target), "対照: 一括再計算の後の A の一覧に F が入らない").toContain(f);
    expect(changed(g), "対照: 一括再計算で G の一覧が変わった").toBe(false);
    for (const id of own) {
      if (changed(id)) expect(notified.has(id), `一覧が変わった記事 ${id} が知らされない`).toBe(true);
      else expect(notified.has(id), `一覧が変わらなかった記事 ${id} が知らされた`).toBe(false);
    }

    // 4. A の一覧の2番目以降の記事を固定すると、A の1件だけが知らされる
    const current = await related(target);
    const pinTarget = current[current.length - 1]!;
    expect(current.indexOf(pinTarget), "対照: 固定する記事が、すでに先頭にある").toBeGreaterThan(0);
    seen = rc.requests.length;
    const group = await cms.translationGroupId(a.editorSession, a.id, pinTarget);
    expectSuccess(await cms.setRelatedRaw(asSession(a.editorSession), a.id, target, { pinned: [group], excluded: [] }), "関連記事の固定");
    await rc.waitFor((r) => relatedEvents([r], path).some((e) => e.entryIds.includes(target)), 60_000);
    await sleep(3);
    const pinEvents = relatedEvents(rc.requests.slice(seen), path);
    expect(pinEvents.flatMap((e) => e.entryIds), "固定を変えた記事のほかにも知らされた").toEqual([target]);

    // 5. 何も変えずにもう一度一括再計算を動かすと、何も知らされない
    seen = rc.requests.length;
    expectSuccess(await cms.recomputeRelatedRaw(asSession(a.adminSession), a.id), "関連記事の一括再計算（2回目）");
    await sleep(15);
    const again = relatedEvents(rc.requests.slice(seen), path).flatMap((e) => e.entryIds);
    expect(again.filter((id) => own.includes(id)), "一覧が変わらないのに知らされた").toEqual([]);
  },
  480_000,
);
