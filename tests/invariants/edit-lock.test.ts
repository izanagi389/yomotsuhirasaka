// 同時編集の約束: INV-LOCK-01
//
// 記事の画面を開いた人が編集のロックを持ち、保存にはロックのIDを付ける（03-content-and-editing.md「同時編集（Q73）」）。
// ほかの人の保存は、最新のリビジョン番号で送っても（楽観ロックでは通る保存でも）、ロックがある間は断られる。
// 「編集を引き継ぐ」をすれば保存でき、引き継がれた側の保存は断られる。
// 断られたことは、状態コードと、下書きの中身が変わっていないことの両方で確かめる。

import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asSession } from "./support/driver.js";
import { expectRefused, expectSuccess } from "./support/assert.js";
import { simpleBody } from "./support/body.js";
import { lazyWorld, marker } from "./support/world.js";

const world = lazyWorld();

inv("inv_lock_01", "他の利用者が編集中の記事は、編集の引き継ぎ操作をしない限り保存できない", async () => {
  const { yomi, a } = await world();
  await yomi.reauthenticate(a.adminSession, a.admin);
  const other = await yomi.createMember(a.adminSession, a.id, "editor");
  const otherSession = await yomi.login(other);
  const { id } = await yomi.createEntry(a.editorSession, a.id, { title: marker("lock01"), body: simpleBody("最初") });
  const bodyOf = async () => JSON.stringify((await yomi.getDraft(a.editorSession, a.id, id)).body);
  const revision = async () => (await yomi.getDraft(a.editorSession, a.id, id)).revision;

  // 編集者が記事の画面を開いて、編集中になる
  const editing = await yomi.acquireEditLock(a.editorSession, a.id, id);
  const mine = marker("lock01-mine");
  expectSuccess(await yomi.saveDraftRaw(asSession(a.editorSession), a.id, id, { body: simpleBody(mine) }, await revision(), { lockId: editing }), "ロックを持つ人の保存");

  // ほかの人は、最新のリビジョン番号でも保存できない（ロックのIDを付けずに / ロックを取ろうとして）
  const blocked = marker("lock01-blocked");
  expectRefused(await yomi.saveDraftRaw(asSession(otherSession), a.id, id, { body: simpleBody(blocked) }, await revision()), "編集中の記事への、ほかの人の保存");
  expectRefused(await yomi.acquireEditLockRaw(asSession(otherSession), a.id, id), "編集中の記事の、ほかの人によるロックの取得");
  expect(await bodyOf(), "断られたはずの保存の内容が入っている").not.toContain(blocked);
  expect(await bodyOf(), "編集中の人の保存の内容が上書きされた").toContain(mine);

  // 編集を引き継げば保存でき、引き継がれた側の保存は断られる
  const takenOver = await yomi.takeOverEditLock(otherSession, a.id, id);
  const theirs = marker("lock01-theirs");
  expectSuccess(await yomi.saveDraftRaw(asSession(otherSession), a.id, id, { body: simpleBody(theirs) }, await revision(), { lockId: takenOver }), "引き継いだ人の保存");
  const stale = marker("lock01-stale");
  expectRefused(
    await yomi.saveDraftRaw(asSession(a.editorSession), a.id, id, { body: simpleBody(stale) }, await revision(), { lockId: editing }),
    "引き継がれた側の、前のロックでの保存",
  );
  expectRefused(await yomi.saveDraftRaw(asSession(a.editorSession), a.id, id, { body: simpleBody(stale) }, await revision()), "引き継がれた側の、ロックのない保存");
  expect(await bodyOf(), "引き継がれた側の保存の内容が入っている").not.toContain(stale);
  expect(await bodyOf(), "引き継いだ人の保存の内容が上書きされた").toContain(theirs);

  // 対照: ロックが外れれば、ロックのIDを付けない保存も通る
  await yomi.releaseEditLock(otherSession, a.id, id, takenOver);
  expectSuccess(await yomi.saveDraftRaw(asSession(a.editorSession), a.id, id, { body: simpleBody(marker("lock01-free")) }, await revision()), "ロックがないときの保存");
});
