// 予約公開の約束: INV-SCHED-01, INV-SCHED-02, INV-SCHED-03
//
// 予約は、その時点の下書きを「予約版」として固定し、指定した時刻に公開する（03-content-and-editing.md「予約公開（Q78）」）。
// 公開されたかどうかは、配信APIで記事のタイトル（一意な目印）を読んで確かめる。予約の時刻は数秒先にし、
// 時刻を過ぎてから公開されるまで（ジョブが取り出すまで）を待つ。
// 「公開されないこと」は、予約の時刻を十分に過ぎてから確かめる（予約がまだ動いていないだけで通らないよう、
// 同じ時刻に予約した対照の記事が公開されたことを待ってから確かめる）。

import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asSession } from "./support/driver.js";
import { expectRefused, expectSuccess } from "./support/assert.js";
import { doc, paragraph } from "./support/body.js";
import { lazyWorld, marker, sleep } from "./support/world.js";

const world = lazyWorld();
/** 予約の時刻（いまから数秒後）。 */
const soon = (seconds = 4) => new Date(Date.now() + seconds * 1000);
/** 予約の時刻を過ぎてから、公開されるまで待つ上限。 */
const PUBLISH_TIMEOUT_MS = 30_000;

/** 配信APIで読んだ、記事の公開中の版のタイトル（公開中でなければ null）。 */
async function deliveredTitle(entryId: string): Promise<string | null> {
  const { yomi, a } = await world();
  const res = await yomi.deliveryRaw(a.id, a.readKey.secret, a.byIdQuery, { id: entryId, locale: "ja" });
  expectSuccess(res, "配信APIでの記事の取得");
  const data = (JSON.parse(res.text) as { data?: Record<string, { title?: string } | null> }).data ?? {};
  return Object.values(data)[0]?.title ?? null;
}

/** 配信APIのタイトルが期待どおりになるまで待つ（なれば true）。 */
async function waitForTitle(entryId: string, title: string, timeoutMs = PUBLISH_TIMEOUT_MS): Promise<boolean> {
  const until = Date.now() + timeoutMs;
  while (Date.now() < until) {
    if ((await deliveredTitle(entryId)) === title) return true;
    await sleep(0.5);
  }
  return false;
}

/** 公開済みの記事を作る。 */
async function publishedEntry(title: string): Promise<string> {
  const { yomi, a } = await world();
  const { id } = await yomi.createEntry(a.editorSession, a.id, { title, body: doc(paragraph("本文")) });
  await yomi.publishEntry(a.editorSession, a.id, id);
  return id;
}

inv("inv_sched_01", "予約公開で公開されるのは予約した時点の内容で、予約後の下書きの編集は反映されない", async () => {
  const { yomi, a } = await world();
  const s = a.editorSession;
  const first = marker("sched01-v1");
  const scheduled = marker("sched01-scheduled");
  const afterwards = marker("sched01-after");
  const id = await publishedEntry(first);

  await yomi.saveDraft(s, a.id, id, { title: scheduled });
  await yomi.schedulePublish(s, a.id, id, soon());
  // 予約の後に下書きを編集する
  await yomi.saveDraft(s, a.id, id, { title: afterwards });
  // 対照: 予約の時刻より前は、前の版のまま
  expect(await deliveredTitle(id), "予約の時刻より前に、予約版が公開されている").toBe(first);

  expect(await waitForTitle(id, scheduled), "予約の時刻を過ぎても、予約した時点の内容が公開されない").toBe(true);
  expect(await deliveredTitle(id), "予約の後の下書きの編集が、公開されている").not.toBe(afterwards);
});

inv("inv_sched_02", "「訂正として公開」は予約できない", async () => {
  const { yomi, a } = await world();
  const s = a.editorSession;
  const first = marker("sched02-v1");
  const corrected = marker("sched02-corrected");
  const id = await publishedEntry(first);
  await yomi.saveDraft(s, a.id, id, { title: corrected });

  const at = soon();
  expectRefused(await yomi.scheduleRaw(asSession(s), a.id, id, at, { correction: true, note: "訂正" }), "訂正として公開の予約");
  // 対照: 同じ時刻に予約した別の記事は公開される（予約の仕組みが動いたことを確かめてから、公開されないことを確かめる）
  const controlFirst = marker("sched02-control-v1");
  const controlNext = marker("sched02-control-v2");
  const control = await publishedEntry(controlFirst);
  await yomi.saveDraft(s, a.id, control, { title: controlNext });
  await yomi.schedulePublish(s, a.id, control, at);
  expect(await waitForTitle(control, controlNext), "対照の記事の予約が公開されない").toBe(true);

  expect(await deliveredTitle(id), "拒否されたはずの訂正の予約が公開されている").toBe(first);
  const versions = await yomi.listVersions(s, a.id, id);
  expect(versions.some((v) => v.isCorrection), "訂正として公開した版が記録されている").toBe(false);
  expect(versions.length, "拒否されたはずの予約で、版が増えている").toBe(1);
});

inv("inv_sched_03", "予約がある記事を「訂正として公開」すると、その記事の予約は取り消され、予約版は公開されない", async () => {
  const { yomi, a } = await world();
  const s = a.editorSession;
  const first = marker("sched03-v1");
  const scheduled = marker("sched03-scheduled");
  const corrected = marker("sched03-corrected");
  const id = await publishedEntry(first);

  const at = soon(5);
  await yomi.saveDraft(s, a.id, id, { title: scheduled });
  await yomi.schedulePublish(s, a.id, id, at);
  // 予約の時刻より前に、訂正として公開する
  await yomi.saveDraft(s, a.id, id, { title: corrected });
  await yomi.publishEntry(s, a.id, id, { correction: true, note: "訂正" });
  expect(await deliveredTitle(id), "訂正として公開した内容が、配信されていない").toBe(corrected);

  // 対照: 同じ時刻に予約した別の記事は公開される
  const controlNext = marker("sched03-control-v2");
  const control = await publishedEntry(marker("sched03-control-v1"));
  await yomi.saveDraft(s, a.id, control, { title: controlNext });
  await yomi.schedulePublish(s, a.id, control, at);
  expect(await waitForTitle(control, controlNext), "対照の記事の予約が公開されない").toBe(true);

  // 予約版は公開されず、訂正した内容のまま
  expect(await deliveredTitle(id), "取り消されたはずの予約版が公開され、訂正が巻き戻っている").toBe(corrected);
  const versions = await yomi.listVersions(s, a.id, id);
  expect(versions.length, "取り消されたはずの予約版が、版として記録されている").toBe(2);
  expect(versions[versions.length - 1]?.isCorrection, "最後の版が、訂正として公開した版ではない").toBe(true);
});
