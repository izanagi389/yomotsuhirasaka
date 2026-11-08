// 「やること」の約束: INV-TODO-01
//
// 「やること」は、その利用者が対応できる項目だけを並べる（03-content-and-editing.md「やることの見せ方（Q67）」）。
// - レビュー待ちは公開できる人（Q77）、未記入の訂正は「訂正と鮮度」の権限を持つ人、原文の更新は記事を編集できる人に出す
// - 閲覧者には何も出さない。他のワークスペースの記事は出さない
// 確認期限切れは、確認期限を最短でも1日先にしか設定できないため、このテストでは作らない。

import { expect } from "vitest";
import { inv } from "./support/inv.js";
import type { YomiClient, Session, TodoItem } from "./support/driver.js";
import { simpleBody } from "./support/body.js";
import { lazyWorld, marker, type WorkspaceFixture } from "./support/world.js";

const world = lazyWorld();

/** 訂正として公開し、説明を書かないままにした記事。 */
async function unwrittenCorrection(yomi: YomiClient, ws: WorkspaceFixture, label: string): Promise<string> {
  const e = await yomi.createEntry(ws.editorSession, ws.id, { title: marker(label), body: simpleBody(marker(label)) });
  await yomi.publishEntry(ws.editorSession, ws.id, e.id);
  await yomi.saveDraft(ws.editorSession, ws.id, e.id, { body: simpleBody(marker(`${label}-fix`)) });
  await yomi.publishEntry(ws.editorSession, ws.id, e.id, { correction: true });
  return e.id;
}

const has = (items: TodoItem[], kind: TodoItem["kind"], entryId: string) => items.some((i) => i.kind === kind && i.entryId === entryId);

inv("inv_todo_01", "「やること」の一覧には、その利用者が対応する権限を持たない項目と、他のワークスペースの項目は含まれない", async () => {
  const { yomi, a, b } = await world();
  await yomi.reauthenticate(a.adminSession, a.admin);
  const author = await yomi.login(await yomi.createMember(a.adminSession, a.id, "author"));
  const viewer = await yomi.login(await yomi.createMember(a.adminSession, a.id, "viewer"));
  const todo = (s: Session, ws: string) => yomi.getTodo(s, ws);

  // A の項目: レビュー待ち、未記入の訂正、原文の更新
  const reviewed = await yomi.createEntry(author, a.id, { title: marker("todo01-review"), body: simpleBody(marker("todo01-review")) });
  await yomi.requestReview(author, a.id, reviewed.id);
  const corrected = await unwrittenCorrection(yomi, a, "todo01-correction");
  const source = await yomi.createEntry(a.editorSession, a.id, { title: marker("todo01-ja"), body: simpleBody(marker("todo01-ja")), locale: "ja" });
  await yomi.publishEntry(a.editorSession, a.id, source.id);
  const translation = await yomi.createTranslation(a.editorSession, a.id, source.id, "en");
  await yomi.saveDraft(a.editorSession, a.id, source.id, { body: simpleBody(marker("todo01-ja2")) });
  await yomi.publishEntry(a.editorSession, a.id, source.id);
  // B の項目: 未記入の訂正
  const correctedInB = await unwrittenCorrection(yomi, b, "todo01-b");

  // 編集者（公開でき、「訂正と鮮度」の権限を持つ）には3つとも出る（対照）
  const editorTodo = await todo(a.editorSession, a.id);
  expect(has(editorTodo, "reviewWaiting", reviewed.id), "編集者の「やること」にレビュー待ちがない").toBe(true);
  expect(has(editorTodo, "unwrittenCorrection", corrected), "編集者の「やること」に未記入の訂正がない").toBe(true);
  expect(has(editorTodo, "sourceUpdated", translation.id), "編集者の「やること」に原文の更新がない").toBe(true);

  // 執筆者には、原文の更新だけが出る（公開できず、「訂正と鮮度」の権限もない）
  const authorTodo = await todo(author, a.id);
  expect(has(authorTodo, "sourceUpdated", translation.id), "執筆者の「やること」に原文の更新がない").toBe(true);
  expect(
    authorTodo.filter((i) => i.kind === "reviewWaiting" || i.kind === "unwrittenCorrection" || i.kind === "reviewExpired"),
    "執筆者の「やること」に、対応する権限のない項目がある",
  ).toEqual([]);

  // 閲覧者には何も出ない
  expect(await todo(viewer, a.id), "閲覧者の「やること」に項目がある").toEqual([]);

  // B の編集者には B の項目だけが出て、A の記事は出ない（逆も同じ）
  const bTodo = await todo(b.editorSession, b.id);
  expect(has(bTodo, "unwrittenCorrection", correctedInB), "B の編集者の「やること」に B の項目がない").toBe(true);
  const aIds = [reviewed.id, corrected, source.id, translation.id];
  expect(bTodo.filter((i) => i.entryId !== null && aIds.includes(i.entryId)), "B の「やること」に A の記事がある").toEqual([]);
  expect(editorTodo.filter((i) => i.entryId === correctedInB), "A の「やること」に B の記事がある").toEqual([]);
});
