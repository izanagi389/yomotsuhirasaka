// 訂正と鮮度の公開の約束: INV-COR-02, INV-COR-03, INV-COR-04, INV-FRS-01
//
// 訂正履歴・削除済みアンカー・鮮度は配信APIの記事の項目（include の corrections・deletedAnchors・freshness）で、
// 訂正フィードは配信APIのルート（correctionFeed）で取る。漏れてはいけない値は一意な目印で見分ける。

import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { driver, type RegisteredQuery } from "./support/driver.js";
import { expectNotLeaked, expectSuccess } from "./support/assert.js";
import { doc, heading, paragraph } from "./support/body.js";
import type { HttpResponse } from "./support/http.js";
import { lazyWorld, marker, sleep } from "./support/world.js";

const world = lazyWorld();

/** 見出し（固定のアンカーID）と本文の段落からなる本文。 */
const sections = (parts: [anchor: string, text: string][]) =>
  doc(...parts.flatMap(([anchor, text]) => [driver.body.withAnchor(heading(`見出し ${anchor}`), anchor), paragraph(text)]));

async function deliveryQuery(include: ("corrections" | "deletedAnchors" | "freshness")[]): Promise<RegisteredQuery> {
  const { cms, a } = await world();
  return cms.registerQuery(a.id, a.registerKey.secret, { kind: "byId", include });
}

async function entryDelivery(query: RegisteredQuery, id: string): Promise<HttpResponse> {
  const { cms, a } = await world();
  const res = await cms.deliveryRaw(a.id, a.readKey.secret, query, { id, locale: "ja" });
  expectSuccess(res, "配信APIでの記事の取得");
  return res;
}

async function feed(): Promise<HttpResponse> {
  const { cms, a } = await world();
  const query = await cms.registerQuery(a.id, a.registerKey.secret, { kind: "correctionFeed" });
  const res = await cms.deliveryRaw(a.id, a.readKey.secret, query, { locale: "ja", limit: 100 });
  expectSuccess(res, "訂正フィードの取得");
  return res;
}

/** 訂正フィードの、ある記事の項目の数。 */
const feedItemsFor = (res: HttpResponse, entryId: string): number => {
  const items = (JSON.parse(res.text) as { data?: { corrections?: { entryId?: string }[] } }).data?.corrections ?? [];
  return items.filter((i) => i.entryId === entryId).length;
};

inv("inv_cor_02", "訂正履歴と削除済みアンカーの応答には、変更前のセクションや削除されたセクションの本文が含まれない", async () => {
  const { cms, a } = await world();
  const s = a.editorSession;
  const before = marker("cor02-before");
  const removed = marker("cor02-removed");
  const after = marker("cor02-after");
  const note = marker("cor02-note");
  const { id } = await cms.createEntry(s, a.id, {
    title: marker("cor02"),
    body: sections([
      ["sec-keep", `残る節 ${before}`],
      ["sec-gone", `消える節 ${removed}`],
    ]),
  });
  await cms.publishEntry(s, a.id, id);
  // 1つ目の節を直し、2つ目の節を消して、訂正として公開する
  await cms.saveDraft(s, a.id, id, { body: sections([["sec-keep", `直した節 ${after}`]]) });
  await cms.publishEntry(s, a.id, id, { correction: true, note });

  const query = await deliveryQuery(["corrections", "deletedAnchors"]);
  const res = await entryDelivery(query, id);
  // 対照: 訂正履歴と削除済みアンカーは返っている
  expect(res.text.includes(note), "訂正履歴に説明がない").toBe(true);
  expect(res.text.includes("sec-gone"), "削除済みアンカーに消えた節がない").toBe(true);
  for (const [what, value] of [["変更前のセクションの本文", before], ["削除されたセクションの本文", removed]] as const) {
    expectNotLeaked(res, value, `記事の訂正履歴と削除済みアンカー（${what}）`);
    expectNotLeaked(await feed(), value, `訂正フィード（${what}）`);
  }
});

inv("inv_cor_03", "訂正フィードには「訂正として公開」したものだけが含まれ、通常の更新や確認済みの記録は含まれない", async () => {
  const { cms, a } = await world();
  const s = a.editorSession;
  // 通常の公開・更新と、確認済みの記録だけの記事
  const { id: plain } = await cms.createEntry(s, a.id, { title: marker("cor03-plain"), body: sections([["p1", "本文"]]) });
  await cms.publishEntry(s, a.id, plain);
  await cms.saveDraft(s, a.id, plain, { body: sections([["p1", "更新した本文"]]) });
  await cms.publishEntry(s, a.id, plain);
  await cms.markVerified(s, a.id, plain, marker("cor03-memo"));
  // 訂正として公開した記事（その後に通常の更新もする）
  const { id: corrected } = await cms.createEntry(s, a.id, { title: marker("cor03-corrected"), body: sections([["c1", "本文"]]) });
  await cms.publishEntry(s, a.id, corrected);
  await cms.saveDraft(s, a.id, corrected, { body: sections([["c1", "訂正した本文"]]) });
  await cms.publishEntry(s, a.id, corrected, { correction: true, note: marker("cor03-note") });
  await cms.saveDraft(s, a.id, corrected, { body: sections([["c1", "訂正の後の通常の更新"]]) });
  await cms.publishEntry(s, a.id, corrected);
  await cms.markVerified(s, a.id, corrected, marker("cor03-memo2"));

  const res = await feed();
  expect(feedItemsFor(res, corrected), "訂正として公開した1回だけが、訂正フィードに載る").toBe(1);
  expect(feedItemsFor(res, plain), "通常の公開・更新・確認済みの記録が、訂正フィードに載っている").toBe(0);
});

inv("inv_cor_04", "訂正の説明文の編集履歴と編集日時は、配信APIに含まれない", async () => {
  const { cms, a } = await world();
  const s = a.editorSession;
  const first = marker("cor04-first");
  const second = marker("cor04-second");
  const { id } = await cms.createEntry(s, a.id, { title: marker("cor04"), body: sections([["s1", "本文"]]) });
  await cms.publishEntry(s, a.id, id);
  await cms.saveDraft(s, a.id, id, { body: sections([["s1", "訂正した本文"]]) });
  await cms.publishEntry(s, a.id, id, { correction: true, note: first });
  const versions = await cms.listVersions(s, a.id, id);
  const correction = versions.find((v) => v.isCorrection);
  expect(correction, "訂正として公開した版がない").toBeDefined();
  // 公開した時刻と、説明を書き直した時刻をずらす
  await sleep(2);
  await cms.editCorrectionNote(s, a.id, id, correction!.id, second);

  const query = await deliveryQuery(["corrections"]);
  for (const [what, res] of [
    ["記事の訂正履歴", await entryDelivery(query, id)],
    ["訂正フィード", await feed()],
  ] as const) {
    // 対照: 書き直した後の説明は出ている
    expect(res.text.includes(second), `${what}に、書き直した後の説明がない`).toBe(true);
    expectNotLeaked(res, first, `${what}（書き直す前の説明）`);
    // 出てよい日時は訂正した日時だけ（説明を書き直した日時を出さない）
    const correctedAt = new Set([...res.text.matchAll(/"correctedAt"\s*:\s*"([^"]+)"/g)].map((m) => m[1]!));
    const timestamps = [...res.text.matchAll(/\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})/g)].map((m) => m[0]);
    for (const t of timestamps) {
      expect(correctedAt.has(t), `${what}に、訂正した日時ではない日時（${t}）がある`).toBe(true);
    }
  }
});

inv("inv_frs_01", "配信APIの鮮度情報には最終確認日だけが含まれ、確認者とメモは含まれない", async () => {
  const { cms, a } = await world();
  const s = a.editorSession;
  const memo = marker("frs01-memo");
  const { id } = await cms.createEntry(s, a.id, { title: marker("frs01"), body: sections([["f1", "本文"]]) });
  await cms.publishEntry(s, a.id, id);
  const query = await deliveryQuery(["freshness"]);
  // 応答の形（{ data: { <記事のフィールド>: { lastVerifiedAt } } }）から最終確認日を読む
  const lastVerified = (res: HttpResponse): string | undefined => {
    const data = (JSON.parse(res.text) as { data?: Record<string, { lastVerifiedAt?: unknown } | null> }).data ?? {};
    const value = Object.values(data)[0]?.lastVerifiedAt;
    return typeof value === "string" ? value : undefined;
  };
  const before = lastVerified(await entryDelivery(query, id));
  await sleep(2);
  await cms.markVerified(s, a.id, id, memo);

  const res = await entryDelivery(query, id);
  // 対照: 最終確認日は出ていて、確認済みの記録で進んでいる
  const after = lastVerified(res);
  expect(typeof after, "最終確認日がない").toBe("string");
  expect(after! > (before ?? ""), "確認済みを記録しても、最終確認日が進んでいない").toBe(true);
  // 確認者（メールアドレスと利用者ID）とメモは出ない
  const me = await cms.me(s);
  for (const [what, value] of [["メモ", memo], ["確認者のメールアドレス", me.email], ["確認者の利用者ID", me.userId]] as const) {
    expectNotLeaked(res, value, `配信APIの鮮度情報（${what}）`);
  }
});
