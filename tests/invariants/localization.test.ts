// 多言語の約束: INV-LOC-01, 03, 04（LOC-05 は取り込みと一緒に import.test.ts）

import { randomUUID } from "node:crypto";
import { expect } from "vitest";
import { inv } from "./support/inv.js";
import type { CmsClient, RegisteredQuery } from "./support/driver.js";
import { simpleBody } from "./support/body.js";
import { expectSuccess } from "./support/assert.js";
import { lazyWorld, marker, type WorkspaceFixture } from "./support/world.js";

const world = lazyWorld();

/** 記事を作って公開する。 */
async function published(cms: CmsClient, ws: WorkspaceFixture, locale: string, label: string, extra: { slug?: string; fields?: Record<string, unknown> } = {}) {
  const mark = marker(label);
  const e = await cms.createEntry(ws.editorSession, ws.id, { title: mark, body: simpleBody(mark), locale, ...extra });
  await cms.publishEntry(ws.editorSession, ws.id, e.id);
  return { id: e.id, mark };
}

/** 翻訳を作り、目印を入れて（必要なら）公開する。 */
async function translation(cms: CmsClient, ws: WorkspaceFixture, sourceId: string, locale: string, label: string, publish: boolean, slug?: string) {
  const t = await cms.createTranslation(ws.editorSession, ws.id, sourceId, locale);
  const mark = marker(label);
  await cms.saveDraft(ws.editorSession, ws.id, t.id, { title: mark, body: simpleBody(mark), ...(slug ? { slug } : {}) });
  if (publish) await cms.publishEntry(ws.editorSession, ws.id, t.id);
  return { id: t.id, mark };
}

inv("inv_loc_01", "あるロケールの記事を編集・公開・訂正・抹消しても、同じ翻訳グループの他のロケールの記事の下書き・公開中の内容・予約は変わらない", async () => {
  const { cms, a } = await world();
  const ja = await published(cms, a, "ja", "loc01-ja");
  const en = await translation(cms, a, ja.id, "en", "loc01-en", true);
  // 公開の後に、enの下書きも変えておく
  const enDraftMarker = marker("loc01-en-draft");
  await cms.saveDraft(a.editorSession, a.id, en.id, { body: simpleBody(enDraftMarker) });

  const enDraftBefore = await cms.getDraft(a.editorSession, a.id, en.id);
  const enVersionsBefore = await cms.listVersions(a.adminSession, a.id, en.id);
  const enDelivered = async () => (await cms.deliveryRaw(a.id, a.readKey.secret, a.byIdQuery, { id: en.id, locale: "en" })).text;
  const enDeliveredBefore = await enDelivered();
  expect(enDeliveredBefore.includes(en.mark), "enの公開版が配信されていない").toBe(true);

  // jaの編集・公開・訂正・抹消
  const jaVersions = await cms.listVersions(a.adminSession, a.id, ja.id);
  expect(jaVersions.length, "jaは1回だけ公開した").toBe(1);
  const jaOldVersion = jaVersions[0]!.id;
  await cms.saveDraft(a.editorSession, a.id, ja.id, { title: marker("loc01-ja2"), body: simpleBody(marker("loc01-ja2")) });
  await cms.publishEntry(a.editorSession, a.id, ja.id);
  await cms.saveDraft(a.editorSession, a.id, ja.id, { body: simpleBody(marker("loc01-ja3")) });
  await cms.publishEntry(a.editorSession, a.id, ja.id, { correction: true });
  await cms.reauthenticate(a.adminSession, a.admin);
  expectSuccess(await cms.eraseVersionRaw(a.adminSession, a.id, ja.id, jaOldVersion, []), "jaの古い版の抹消");

  // enは何も変わらない
  const enDraftAfter = await cms.getDraft(a.editorSession, a.id, en.id);
  expect(enDraftAfter, "enの下書きが変わった").toEqual(enDraftBefore);
  expect(await cms.listVersions(a.adminSession, a.id, en.id), "enの版の履歴が変わった").toEqual(enVersionsBefore);
  expect(await enDelivered(), "enの公開中の内容が変わった").toBe(enDeliveredBefore);
  // 予約（M2）は、予約公開が入ったときにこのテストで確かめる範囲を広げる
});

inv("inv_loc_03", "配信APIが返す他のロケールへのリンクには、公開中の記事だけが含まれる", async () => {
  const { cms, a } = await world();
  const slug = (l: string) => `loc03-${l}-${randomUUID().slice(0, 8)}`;
  const ja = await published(cms, a, "ja", "loc03-ja", { slug: slug("ja") });
  const enSlug = slug("en");
  const frSlug = slug("fr");
  const en = await translation(cms, a, ja.id, "en", "loc03-en", false, enSlug); // 下書きのまま
  const fr = await translation(cms, a, ja.id, "fr", "loc03-fr", true, frSlug); // 公開
  const alternates: RegisteredQuery = await cms.registerQuery(a.id, a.registerKey.secret, { kind: "byId", include: ["alternates"] });
  const links = async () => (await cms.deliveryRaw(a.id, a.readKey.secret, alternates, { id: ja.id, locale: "ja" })).text;

  let text = await links();
  expect(text.includes(frSlug), "公開中のfrへのリンクがない").toBe(true);
  expect(text.includes(enSlug) || text.includes(en.id), "下書きのままのenへのリンクが含まれている").toBe(false);

  await cms.publishEntry(a.editorSession, a.id, en.id);
  text = await links();
  expect(text.includes(enSlug), "公開したenへのリンクがない").toBe(true);

  await cms.unpublishEntry(a.editorSession, a.id, fr.id);
  text = await links();
  expect(text.includes(frSlug) || text.includes(fr.id), "非公開にしたfrへのリンクが含まれている").toBe(false);
});

inv("inv_loc_04", "参照フィールドは配信時に同じロケールの記事へ解決され、他のロケールの記事や公開されていない記事は返らない", async () => {
  const { cms, a } = await world();
  const groupOf = async (id: string) => (await cms.getTranslationInfo(a.editorSession, a.id, id)).groupId;

  // Q: ja・en とも公開 / R: en だけ公開 / S: ja は下書きのまま
  const qJa = await published(cms, a, "ja", "loc04-q-ja");
  const qEn = await translation(cms, a, qJa.id, "en", "loc04-q-en", true);
  const rEn = await published(cms, a, "en", "loc04-r-en");
  const sJaMark = marker("loc04-s-ja");
  const sJa = await cms.createEntry(a.editorSession, a.id, { title: sJaMark, body: simpleBody(sJaMark), locale: "ja" });

  const related = [await groupOf(qJa.id), await groupOf(rEn.id), await groupOf(sJa.id)];
  const p = await published(cms, a, "ja", "loc04-p", { fields: { related } });
  const query = await cms.registerQuery(a.id, a.registerKey.secret, { kind: "byId", include: ["related"] });
  const resolve = async () => (await cms.deliveryRaw(a.id, a.readKey.secret, query, { id: p.id, locale: "ja" })).text;

  let text = await resolve();
  expect(text.includes(qJa.mark), "同じロケール（ja）の参照先が返っていない").toBe(true);
  for (const [label, m] of [["他のロケール（en）のQ", qEn.mark], ["jaのないR（en）", rEn.mark], ["公開されていないS", sJaMark]] as const) {
    expect(text.includes(m), `${label}が参照先として返った`).toBe(false);
  }

  // 参照先を非公開にすると返らない
  await cms.unpublishEntry(a.editorSession, a.id, qJa.id);
  text = await resolve();
  expect(text.includes(qJa.mark), "非公開にした参照先が返った").toBe(false);
  expect(text.includes(qEn.mark), "非公開にした参照先の代わりに他のロケールが返った").toBe(false);
});
