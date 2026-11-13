// 公開・版・権限の約束: INV-PUB-01〜06, INV-PERM-01, INV-IMG-01〜03, INV-COR-01, INV-URL-01, INV-EDIT-01

import { randomUUID } from "node:crypto";
import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asBearer, asSession, driver, type Session } from "./support/driver.js";
import { expectNotLeaked, expectRefused, expectRejected, expectSuccess, expectTextNotLeaked } from "./support/assert.js";
import { doc, paragraph, simpleBody } from "./support/body.js";
import { pngImage } from "./support/images.js";
import { operationsOf } from "./support/operations.js";
import { beforeSaveSettingImage } from "./support/scripts.js";
import { BASE_TEMPLATE, cloneTemplate, postType } from "./support/template.js";
import { lazyWorld, marker, type World, type WorkspaceFixture } from "./support/world.js";

const world = lazyWorld();

/** 配信APIで、記事1件（ID指定）と一覧の応答を取る。 */
async function delivered(w: World, ws: WorkspaceFixture, entryId: string): Promise<{ byId: string; list: string }> {
  const byId = await w.yomi.deliveryRaw(ws.id, ws.readKey.secret, ws.byIdQuery, { id: entryId, locale: "ja" });
  const list = await w.yomi.deliveryRaw(ws.id, ws.readKey.secret, ws.listQuery, { locale: "ja", limit: postType(BASE_TEMPLATE).listLimit });
  return { byId: byId.text, list: list.text };
}

/** 本文に目印を含む版のID。 */
async function versionWith(w: World, session: Session, ws: WorkspaceFixture, entryId: string, mark: string): Promise<string> {
  for (const v of await w.yomi.listVersions(session, ws.id, entryId)) {
    const res = await w.yomi.getVersionRaw(asSession(session), ws.id, entryId, v.id);
    if (res.text.includes(mark)) return v.id;
  }
  throw new Error(`目印 ${mark} を含む版が見つからない`);
}

async function publishedEntry(w: World, ws: WorkspaceFixture, mark: string): Promise<string> {
  const e = await w.yomi.createEntry(ws.editorSession, ws.id, { title: `記事 ${mark}`, body: simpleBody(mark) });
  await w.yomi.publishEntry(ws.editorSession, ws.id, e.id);
  return e.id;
}

inv("inv_pub_01", "配信APIは公開版だけを返し、下書きの内容を返さない", async () => {
  const w = await world();
  const { yomi, a } = w;

  // 公開した後に下書きを変えても、配信は公開版のまま
  const published = marker("pub01-published");
  const draft = marker("pub01-draft");
  const id = await publishedEntry(w, a, published);
  await yomi.saveDraft(a.editorSession, a.id, id, { title: draft, body: simpleBody(draft) });
  const d1 = await delivered(w, a, id);
  expect(d1.byId.includes(published), "公開版が配信されていない").toBe(true);
  expectTextNotLeaked(d1.byId + d1.list, draft, "公開後に変えた下書きの、配信APIの応答");

  // 一度も公開していない記事は配信されない
  const neverMarker = marker("pub01-never");
  const never = await yomi.createEntry(a.editorSession, a.id, { title: neverMarker, body: simpleBody(neverMarker) });
  const d2 = await delivered(w, a, never.id);
  expect((d2.byId + d2.list).includes(neverMarker), "公開していない記事が配信された").toBe(false);
});

inv("inv_pub_02", "記事を非公開にすると、その直後からCMSの配信APIで取得できなくなる", async () => {
  const w = await world();
  const mark = marker("pub02");
  const id = await publishedEntry(w, w.a, mark);
  expect((await delivered(w, w.a, id)).byId.includes(mark), "非公開にする前に配信されていない").toBe(true);

  await w.yomi.unpublishEntry(w.a.editorSession, w.a.id, id);
  // 待たずに確かめる
  const after = await delivered(w, w.a, id);
  expect((after.byId + after.list).includes(mark), "非公開にした直後も配信された").toBe(false);
});

inv("inv_pub_03", "公開版を更新しても、以前の版は版の履歴として残る（本文を抹消した版は、IDとメタデータだけが残る。テンプレートを切り替えた版は、本文が新しいテンプレートの形に置き換わるが、IDとメタデータは残る）", async () => {
  const w = await world();
  const { yomi, a } = w;
  const v1Marker = marker("pub03-v1");
  const v2Marker = marker("pub03-v2");
  const id = await publishedEntry(w, a, v1Marker);
  await yomi.saveDraft(a.editorSession, a.id, id, { body: simpleBody(v2Marker) });
  await yomi.publishEntry(a.editorSession, a.id, id);

  const versions = await yomi.listVersions(a.adminSession, a.id, id);
  expect(versions.length, "版の履歴の数").toBeGreaterThanOrEqual(2);
  const v1 = await versionWith(w, a.adminSession, a, id, v1Marker);
  await versionWith(w, a.adminSession, a, id, v2Marker);

  // 以前の版の本文を抹消しても、版そのもの（ID）は履歴に残る
  await yomi.reauthenticate(a.adminSession, a.admin);
  expectSuccess(await yomi.eraseVersionRaw(a.adminSession, a.id, id, v1, []), "以前の版の抹消");
  const after = await yomi.listVersions(a.adminSession, a.id, id);
  const erased = after.find((v) => v.id === v1);
  expect(erased, "抹消した版が履歴から消えた").toBeDefined();
  expect(erased!.erased, "抹消した版に抹消の印がない").toBe(true);
  expect(after.length, "抹消で版の数が変わった").toBe(versions.length);
  const meta = await yomi.getVersionRaw(asSession(a.adminSession), a.id, id, v1);
  expectSuccess(meta, "抹消した版のメタデータの取得");
});

inv("inv_pub_04", "本文を抹消した版の本文は、どのAPIの応答にも含まれない", async () => {
  const w = await world();
  const { yomi, a } = w;
  const erasedMarker = marker("pub04-erased");
  const id = await publishedEntry(w, a, erasedMarker);
  await yomi.saveDraft(a.editorSession, a.id, id, { title: "差し替え後", body: simpleBody(marker("pub04-current")) });
  await yomi.publishEntry(a.editorSession, a.id, id);
  const v1 = await versionWith(w, a.adminSession, a, id, erasedMarker);
  await yomi.reauthenticate(a.adminSession, a.admin);
  expectSuccess(await yomi.eraseVersionRaw(a.adminSession, a.id, id, v1, []), "版の抹消");

  // 版の取得
  expectNotLeaked(await yomi.getVersionRaw(asSession(a.adminSession), a.id, id, v1), erasedMarker, "抹消した版の取得");
  // 管理APIの読み取りの操作すべて（対象をこの記事にして、管理者とサーバー管理者で）
  const target = { workspaceId: a.id, entryId: id, userId: a.editor.userId, apiKeyId: a.readKey.id };
  for (const session of [a.adminSession, w.serverAdminSession]) {
    for (const op of operationsOf(yomi).filter((o) => !o.mutating)) {
      expectNotLeaked(await op.call(asSession(session), target), erasedMarker, `抹消の後の ${op.name}`);
    }
  }
  // 配信API
  for (const res of [
    await yomi.deliveryRaw(a.id, a.readKey.secret, a.byIdQuery, { id, locale: "ja" }),
    await yomi.deliveryRaw(a.id, a.readKey.secret, a.listQuery, { locale: "ja", limit: 100 }),
  ]) {
    expectNotLeaked(res, erasedMarker, "抹消の後の配信API");
  }
});

inv("inv_pub_05", "現在公開中の版の本文は抹消できない", async () => {
  const w = await world();
  const { yomi, a } = w;
  const oldMarker = marker("pub05-old");
  const currentMarker = marker("pub05-current");
  const id = await publishedEntry(w, a, oldMarker);
  await yomi.saveDraft(a.editorSession, a.id, id, { body: simpleBody(currentMarker) });
  await yomi.publishEntry(a.editorSession, a.id, id);
  const current = await versionWith(w, a.adminSession, a, id, currentMarker);
  const old = await versionWith(w, a.adminSession, a, id, oldMarker);
  await yomi.reauthenticate(a.adminSession, a.admin);

  expectRefused(await yomi.eraseVersionRaw(a.adminSession, a.id, id, current, []), "公開中の版の抹消");
  expect((await delivered(w, a, id)).byId.includes(currentMarker), "公開中の版の本文が消えた").toBe(true);
  const versions = await yomi.listVersions(a.adminSession, a.id, id);
  expect(versions.find((v) => v.id === current)?.erased, "公開中の版に抹消の印が付いた").toBe(false);

  // 対照: 公開中でない版は抹消できる。非公開にした後なら、元の公開中の版も抹消できる
  expectSuccess(await yomi.eraseVersionRaw(a.adminSession, a.id, id, old, []), "公開中でない版の抹消");
  await yomi.unpublishEntry(a.adminSession, a.id, id);
  await yomi.reauthenticate(a.adminSession, a.admin);
  expectSuccess(await yomi.eraseVersionRaw(a.adminSession, a.id, id, current, []), "非公開にした後の、元の公開中の版の抹消");
});

inv(
  "inv_pub_06",
  "抹消の際に削除を選んだメディアは、そのURLを指定しても取得できなくなる。ワークスペース内のどこかの記事の版や下書きで使われているメディアは、削除の候補に含まれない",
  async () => {
    const w = await world();
    const { yomi, a } = w;
    const s = a.editorSession;
    const upload = async (name: string) => yomi.uploadMedia(s, a.id, { filename: `${name}.png`, bytes: pngImage(8, 8), contentType: "image/png" });
    const onlyHere = await upload("only-here"); // この版だけで使う
    const inOtherDraft = await upload("other-draft"); // 別の記事の下書きでも使う
    const inCurrent = await upload("current"); // この記事の公開中の版でも使う
    const inOtherOldVersion = await upload("other-old"); // 別の記事の古い版でも使う
    const inOtherCoverDraft = await upload("other-cover-draft"); // 別の記事の下書きの画像のフィールドでも使う
    const inOtherCoverOld = await upload("other-cover-old"); // 別の記事の古い版の画像のフィールドでも使う
    const img = (m: { id: string }) => driver.body.image({ mediaId: m.id, alt: "画像" });
    const cover = (m: { id: string }) => ({ cover: driver.fields.image({ mediaId: m.id, alt: "画像" }) });

    // 別の記事: 古い版で inOtherOldVersion を使い、その後の版では使わない。下書きで inOtherDraft を使う
    const other = await yomi.createEntry(s, a.id, { title: marker("pub06-other"), body: doc(paragraph("x"), img(inOtherOldVersion)) });
    await yomi.publishEntry(s, a.id, other.id);
    await yomi.saveDraft(s, a.id, other.id, { body: doc(paragraph("y")) });
    await yomi.publishEntry(s, a.id, other.id);
    await yomi.saveDraft(s, a.id, other.id, { body: doc(paragraph("z"), img(inOtherDraft)) });

    // さらに別の記事: 画像のフィールドで、古い版では inOtherCoverOld を、下書きでは inOtherCoverDraft を使う
    const coverUser = await yomi.createEntry(s, a.id, { title: marker("pub06-cover"), body: doc(paragraph("c")), fields: cover(inOtherCoverOld) });
    await yomi.publishEntry(s, a.id, coverUser.id);
    await yomi.saveDraft(s, a.id, coverUser.id, { fields: { cover: null } });
    await yomi.publishEntry(s, a.id, coverUser.id);
    await yomi.saveDraft(s, a.id, coverUser.id, { fields: cover(inOtherCoverDraft) });

    // 対象の記事: v1 ですべて使い、v2（公開中）では inCurrent だけを使う
    const v1Marker = marker("pub06-v1");
    const e = await yomi.createEntry(s, a.id, { title: "抹消の対象", body: doc(paragraph(v1Marker), img(onlyHere), img(inOtherDraft), img(inCurrent), img(inOtherOldVersion), img(inOtherCoverDraft), img(inOtherCoverOld)) });
    await yomi.publishEntry(s, a.id, e.id);
    await yomi.saveDraft(s, a.id, e.id, { body: doc(paragraph("v2"), img(inCurrent)) });
    await yomi.publishEntry(s, a.id, e.id);
    const v1 = await versionWith(w, a.adminSession, a, e.id, v1Marker);

    const candidates = await yomi.eraseCandidates(a.adminSession, a.id, e.id, v1);
    expect(candidates, "他で使われていないメディアが削除の候補にない").toContain(onlyHere.id);
    for (const [label, m] of [
      ["別の記事の下書き", inOtherDraft],
      ["この記事の公開中の版", inCurrent],
      ["別の記事の古い版", inOtherOldVersion],
      ["別の記事の下書きの画像のフィールド", inOtherCoverDraft],
      ["別の記事の古い版の画像のフィールド", inOtherCoverOld],
    ] as const) {
      expect(candidates, `${label}で使われているメディアが削除の候補に入っている`).not.toContain(m.id);
    }

    // 削除を選んだメディアは取得できなくなる。使われているメディアは、指定されても消えない
    await yomi.reauthenticate(a.adminSession, a.admin);
    await yomi.eraseVersionRaw(a.adminSession, a.id, e.id, v1, [onlyHere.id, inOtherDraft.id, inOtherCoverDraft.id]);
    const gone = await yomi.fetchMediaRaw(onlyHere.url);
    expect(gone.status >= 400 && gone.status < 500, `削除を選んだメディアが取得できる（状態コード ${gone.status}）`).toBe(true);
    for (const m of [inOtherDraft, inCurrent, inOtherOldVersion, inOtherCoverDraft, inOtherCoverOld]) {
      expectSuccess(await yomi.fetchMediaRaw(m.url), "使われているメディアの取得");
    }
  },
  120_000,
);

inv("inv_perm_01", "執筆者のロールでは記事を公開できない", async () => {
  const w = await world();
  const { yomi, a } = w;
  const author = await yomi.createMember(a.adminSession, a.id, "author");
  const session = await yomi.login(author);
  const mark = marker("perm01");
  const e = await yomi.createEntry(session, a.id, { title: mark, body: simpleBody(mark) });

  expectRejected(await yomi.publishRaw(asSession(session), a.id, e.id), "執筆者による公開");
  expectRejected(await yomi.publishRaw(asSession(session), a.id, e.id, { correction: true }), "執筆者による訂正としての公開");
  const publish = operationsOf(yomi).filter((o) => o.name === "publishEntry" || o.name === "publishAsCorrection");
  const target = { workspaceId: a.id, entryId: e.id, userId: author.userId, apiKeyId: a.readKey.id };
  for (const op of publish) expectRejected(await op.call(asSession(session), target), `執筆者による ${op.name}`);
  const d = await delivered(w, a, e.id);
  expect((d.byId + d.list).includes(mark), "執筆者の操作で公開されてしまった").toBe(false);

  // 対照: 編集者は公開できる
  expectSuccess(await yomi.publishRaw(asSession(a.editorSession), a.id, e.id), "編集者による公開");
});

inv("inv_img_01", "代替テキストも「装飾画像」の印もない画像を含む記事は公開できない", async () => {
  const w = await world();
  const { yomi, a } = w;
  const s = a.editorSession;
  const media = await yomi.uploadMedia(s, a.id, { filename: "alt.png", bytes: pngImage(8, 8), contentType: "image/png" });
  const create = async (label: string, ...images: ReturnType<typeof driver.body.image>[]) => {
    const mark = marker(label);
    const e = await yomi.createEntry(s, a.id, { title: mark, body: doc(paragraph(mark), ...images) });
    return { id: e.id, mark };
  };

  for (const [label, images] of [
    ["代替テキストも印もない", [driver.body.image({ mediaId: media.id })]],
    ["空の代替テキストで印もない", [driver.body.image({ mediaId: media.id, alt: "", decorative: false })]],
    ["2枚のうち1枚だけ代替テキストがない", [driver.body.image({ mediaId: media.id, alt: "説明" }), driver.body.image({ mediaId: media.id })]],
  ] as const) {
    const e = await create("img01-ng", ...images);
    expectRefused(await yomi.publishRaw(asSession(s), a.id, e.id), `${label}画像を含む記事の公開`);
    const d = await delivered(w, a, e.id);
    expect((d.byId + d.list).includes(e.mark), `${label}画像を含む記事が公開された`).toBe(false);
  }

  // 対照: 代替テキストがあるか、装飾画像の印があれば公開できる
  const withAlt = await create("img01-alt", driver.body.image({ mediaId: media.id, alt: "説明" }));
  expectSuccess(await yomi.publishRaw(asSession(s), a.id, withAlt.id), "代替テキストのある画像を含む記事の公開");
  const decorative = await create("img01-deco", driver.body.image({ mediaId: media.id, decorative: true }));
  expectSuccess(await yomi.publishRaw(asSession(s), a.id, decorative.id), "装飾画像の印のある画像を含む記事の公開");

  // 画像のフィールド（サムネイルなど）の画像も同じ
  const withCover = async (label: string, image: Record<string, unknown>) => {
    const mark = marker(label);
    const e = await yomi.createEntry(s, a.id, { title: mark, body: doc(paragraph(mark)), fields: { cover: image } });
    return { id: e.id, mark };
  };
  for (const [label, image] of [
    ["代替テキストも印もない", driver.fields.image({ mediaId: media.id })],
    ["空の代替テキストで印もない", driver.fields.image({ mediaId: media.id, alt: " ", decorative: false })],
  ] as const) {
    const e = await withCover("img01-cover-ng", image);
    expectRefused(await yomi.publishRaw(asSession(s), a.id, e.id), `画像のフィールドに${label}画像がある記事の公開`);
    const d = await delivered(w, a, e.id);
    expect((d.byId + d.list).includes(e.mark), `画像のフィールドに${label}画像がある記事が公開された`).toBe(false);
  }
  const coverAlt = await withCover("img01-cover-alt", driver.fields.image({ mediaId: media.id, alt: "説明" }));
  expectSuccess(await yomi.publishRaw(asSession(s), a.id, coverAlt.id), "画像のフィールドに代替テキストのある画像がある記事の公開");
  const coverDeco = await withCover("img01-cover-deco", driver.fields.image({ mediaId: media.id, decorative: true }));
  expectSuccess(await yomi.publishRaw(asSession(s), a.id, coverDeco.id), "画像のフィールドに装飾画像の印のある画像がある記事の公開");
});

inv("inv_img_02", "画像のフィールドが指すメディアが削除されると、配信APIはそのフィールドに画像を返さない", async () => {
  const w = await world();
  const { yomi, a } = w;
  const s = a.editorSession;
  const query = await yomi.registerQuery(a.id, a.registerKey.secret, { kind: "byId", fields: ["id", "title"], include: ["cover"] });
  const listQuery = await yomi.registerQuery(a.id, a.registerKey.secret, { kind: "list", fields: ["id", "title"], include: ["cover"] });
  const coverOf = async (entryId: string) => {
    const res = await yomi.deliveryRaw(a.id, a.readKey.secret, query, { id: entryId, locale: "ja" });
    expectSuccess(res, "配信APIでの記事の取得");
    return { image: driver.fields.deliveredImage(res.text, "cover", entryId), text: res.text };
  };
  // 一覧（新しい順）。このテストで公開した記事が最も新しいので、一覧の上限に入る
  const listedCoverOf = async (entryId: string) => {
    const res = await yomi.deliveryRaw(a.id, a.readKey.secret, listQuery, { locale: "ja", limit: postType(BASE_TEMPLATE).listLimit });
    expectSuccess(res, "配信APIでの一覧の取得");
    return { image: driver.fields.deliveredImage(res.text, "cover", entryId), text: res.text };
  };
  const pathOf = (url: string) => new URL(url).pathname;

  const media = await yomi.uploadMedia(s, a.id, { filename: "cover.png", bytes: pngImage(8, 8), contentType: "image/png" });
  const kept = await yomi.uploadMedia(s, a.id, { filename: "kept.png", bytes: pngImage(8, 8), contentType: "image/png" });
  const mark = marker("img02");
  const e = await yomi.createEntry(s, a.id, { title: mark, body: doc(paragraph(mark)), fields: { cover: driver.fields.image({ mediaId: media.id, alt: "表紙" }) } });
  await yomi.publishEntry(s, a.id, e.id);
  const control = await yomi.createEntry(s, a.id, { title: marker("img02-control"), body: doc(paragraph("x")), fields: { cover: driver.fields.image({ mediaId: kept.id, alt: "表紙" }) } });
  await yomi.publishEntry(s, a.id, control.id);

  // 対照: 削除する前は、画像のフィールドにメディアのURLが返る
  const before = await coverOf(e.id);
  expect(before.image, "削除する前の画像のフィールドが空").not.toBeNull();
  expect(pathOf(before.image!.url), "画像のフィールドのURLが、アップロードしたメディアのURLでない").toBe(pathOf(media.url));
  expect(before.image!.alt, "画像のフィールドの代替テキストが返らない").toBe("表紙");
  const listedBefore = await listedCoverOf(e.id);
  expect(listedBefore.image && pathOf(listedBefore.image.url), "削除する前の一覧で、画像のフィールドにメディアのURLが返らない").toBe(pathOf(media.url));

  // メディアを削除する（使われていても削除する）
  expectSuccess(await yomi.deleteMediaRaw(a.adminSession, a.id, media.id, { force: true }), "使われているメディアの削除");

  const after = await coverOf(e.id);
  expect(after.image, "削除したメディアが、画像のフィールドに返った").toBeNull();
  expect(after.text.includes(pathOf(media.url)), "削除したメディアのURLが、配信APIの応答に含まれる").toBe(false);
  expect(after.text.includes(mark), "画像を返さない代わりに、記事ごと返らなくなった").toBe(true);
  const listedAfter = await listedCoverOf(e.id);
  expect(listedAfter.image, "削除したメディアが、一覧の画像のフィールドに返った").toBeNull();
  expect(listedAfter.text.includes(pathOf(media.url)), "削除したメディアのURLが、一覧の応答に含まれる").toBe(false);

  // 対照: 削除していないメディアは、ほかの記事で返り続ける
  const other = await coverOf(control.id);
  expect(other.image && pathOf(other.image.url), "削除していないメディアが、画像のフィールドに返らない").toBe(pathOf(kept.url));
});

inv("inv_img_03", "画像のフィールドには、そのワークスペースにある削除されていないメディアしか保存できない", async () => {
  const w = await world();
  const { yomi, a, b } = w;
  const s = a.editorSession;
  const png = () => ({ filename: "img03.png", bytes: pngImage(8, 8), contentType: "image/png" });
  const own = await yomi.uploadMedia(s, a.id, png());
  const deleted = await yomi.uploadMedia(s, a.id, png());
  expectSuccess(await yomi.deleteMediaRaw(a.adminSession, a.id, deleted.id), "使われていないメディアの削除");
  const otherWorkspace = await yomi.uploadMedia(b.editorSession, b.id, png());
  const cover = (mediaId: string) => ({ cover: driver.fields.image({ mediaId, alt: "表紙" }) });

  const target = await yomi.createEntry(s, a.id, { title: marker("img03"), body: doc(paragraph("x")) });
  for (const [label, mediaId] of [
    ["存在しない", randomUUID()],
    ["削除した", deleted.id],
    ["ほかのワークスペースの", otherWorkspace.id],
  ] as const) {
    expectRefused(await yomi.createEntryRaw(asSession(s), a.id, { title: marker("img03-new"), body: doc(paragraph("x")), fields: cover(mediaId) }), `${label}メディアを画像のフィールドに入れた記事の作成`);
    const draft = await yomi.getDraft(s, a.id, target.id);
    expectRefused(await yomi.saveDraftRaw(asSession(s), a.id, target.id, { fields: cover(mediaId) }, draft.revision), `${label}メディアを画像のフィールドに入れた下書きの保存`);
  }
  const draft = await yomi.getDraft(s, a.id, target.id);
  expect(JSON.stringify(draft.fields["cover"] ?? null).includes(otherWorkspace.id), "ほかのワークスペースのメディアが、下書きに保存された").toBe(false);

  // 対照: 同じワークスペースの削除されていないメディアは保存できる
  expectSuccess(await yomi.saveDraftRaw(asSession(s), a.id, target.id, { fields: cover(own.id) }, draft.revision), "同じワークスペースのメディアを画像のフィールドに入れた下書きの保存");

  // 保存前のフックが画像のフィールドを直した場合も同じ（フックの直しも保存の前に確かめる）
  const { serverAdmin, serverAdminSession } = w;
  const name = `inv-img03-${randomUUID().slice(0, 8)}`;
  const hooked = cloneTemplate(name, (t) => (postType(t).hooks = { beforeSave: { source: beforeSaveSettingImage("cover"), permissions: [] } }));
  expectSuccess(await yomi.deployTemplateRaw(asBearer(await yomi.deployToken(serverAdminSession, serverAdmin)), hooked), "テンプレートのデプロイ");
  await yomi.reauthenticate(serverAdminSession, serverAdmin);
  const site = await yomi.createWorkspace(serverAdminSession, `${name}-ws`, { template: name });
  const siteAdmin = await yomi.createMember(serverAdminSession, site.id, "admin");
  const siteAdminSession = await yomi.login(siteAdmin);
  const siteEditor = await yomi.login(await yomi.createMember(siteAdminSession, site.id, "editor"));
  const siteOwn = await yomi.uploadMedia(siteEditor, site.id, png());
  const siteDeleted = await yomi.uploadMedia(siteEditor, site.id, png());
  expectSuccess(await yomi.deleteMediaRaw(siteAdminSession, site.id, siteDeleted.id), "使われていないメディアの削除");
  const hookTarget = await yomi.createEntry(siteEditor, site.id, { title: marker("img03-hook"), body: doc(paragraph("x")) });
  for (const [label, mediaId] of [
    ["存在しない", randomUUID()],
    ["削除した", siteDeleted.id],
    ["ほかのワークスペースの", otherWorkspace.id],
  ] as const) {
    const d = await yomi.getDraft(siteEditor, site.id, hookTarget.id);
    expectRefused(
      await yomi.saveDraftRaw(asSession(siteEditor), site.id, hookTarget.id, { fields: { summary: `media:${mediaId}` } }, d.revision),
      `保存前のフックが${label}メディアを画像のフィールドに入れる下書きの保存`,
    );
  }
  const hookDraft = await yomi.getDraft(siteEditor, site.id, hookTarget.id);
  expect(JSON.stringify(hookDraft.fields["cover"] ?? null).includes(otherWorkspace.id), "保存前のフックが入れたほかのワークスペースのメディアが、下書きに保存された").toBe(false);
  // 対照: フックが同じワークスペースのメディアを入れるなら保存でき、フックが動いたことも確かめる
  expectSuccess(
    await yomi.saveDraftRaw(asSession(siteEditor), site.id, hookTarget.id, { fields: { summary: `media:${siteOwn.id}` } }, hookDraft.revision),
    "保存前のフックが同じワークスペースのメディアを画像のフィールドに入れる下書きの保存",
  );
  const saved = await yomi.getDraft(siteEditor, site.id, hookTarget.id);
  expect(JSON.stringify(saved.fields["cover"] ?? null).includes(siteOwn.id), "保存前のフックが動いていない（画像のフィールドが直っていない）").toBe(true);
}, 120_000);

inv("inv_cor_01", "「訂正として公開」した版は訂正として記録され、通常の公開と区別して取得できる", async () => {
  const w = await world();
  const { yomi, a } = w;
  const normalMarker = marker("cor01-normal");
  const correctedMarker = marker("cor01-corrected");
  const id = await publishedEntry(w, a, normalMarker);
  await yomi.saveDraft(a.editorSession, a.id, id, { body: simpleBody(correctedMarker) });
  await yomi.publishEntry(a.editorSession, a.id, id, { correction: true });

  const normal = await versionWith(w, a.adminSession, a, id, normalMarker);
  const corrected = await versionWith(w, a.adminSession, a, id, correctedMarker);
  const versions = await yomi.listVersions(a.adminSession, a.id, id);
  expect(versions.find((v) => v.id === corrected)?.isCorrection, "訂正として公開した版に、訂正の印がない").toBe(true);
  expect(versions.find((v) => v.id === normal)?.isCorrection, "通常の公開の版に、訂正の印が付いている").toBe(false);
});

inv("inv_url_01", "記事の公開URLが変わると、変更前と変更後のURLの対応が変更履歴に記録される", async () => {
  const w = await world();
  const { yomi, a } = w;
  const template = postType(BASE_TEMPLATE).urlTemplate;
  const urlOf = (slug: string) => template.replace("{locale}", "ja").replace("{slug}", slug);
  const slug1 = `url01-${randomUUID().slice(0, 8)}`;
  const slug2 = `${slug1}-renamed`;
  const slug3 = `${slug1}-again`;

  const e = await yomi.createEntry(a.editorSession, a.id, { title: "URLの変更", body: simpleBody("x"), slug: slug1 });
  await yomi.publishEntry(a.editorSession, a.id, e.id);
  await yomi.saveDraft(a.editorSession, a.id, e.id, { slug: slug2 });
  await yomi.publishEntry(a.editorSession, a.id, e.id);
  await yomi.saveDraft(a.editorSession, a.id, e.id, { slug: slug3 });
  await yomi.publishEntry(a.editorSession, a.id, e.id);

  const history = await yomi.getUrlHistory(a.editorSession, a.id, e.id);
  expect(history, "1回目の変更が記録されていない").toContainEqual({ from: urlOf(slug1), to: urlOf(slug2) });
  expect(history, "2回目の変更が記録されていない").toContainEqual({ from: urlOf(slug2), to: urlOf(slug3) });
});

inv("inv_edit_01", "保存時に照合するリビジョン番号が最新でない下書きの保存は拒否され、他の人の保存内容を上書きしない", async () => {
  const w = await world();
  const { yomi, a } = w;
  const other = await yomi.createMember(a.adminSession, a.id, "editor");
  const otherSession = await yomi.login(other);
  const e = await yomi.createEntry(a.editorSession, a.id, { title: "同時編集", body: simpleBody("最初") });

  const base = await yomi.getDraft(a.editorSession, a.id, e.id);
  const firstMarker = marker("edit01-first");
  const secondMarker = marker("edit01-second");
  expectSuccess(await yomi.saveDraftRaw(asSession(a.editorSession), a.id, e.id, { body: simpleBody(firstMarker) }, base.revision), "最初の保存");

  // 同じリビジョン番号を元にした、別の人の保存
  const conflict = await yomi.saveDraftRaw(asSession(otherSession), a.id, e.id, { body: simpleBody(secondMarker) }, base.revision);
  expect(conflict.status, "古いリビジョン番号での保存の状態コード").toBe(409);
  const now = await yomi.getDraft(a.editorSession, a.id, e.id);
  expect(JSON.stringify(now.body).includes(firstMarker), "最初の保存の内容が上書きされた").toBe(true);
  expect(JSON.stringify(now.body).includes(secondMarker), "拒否された保存の内容が入っている").toBe(false);

  // 対照: 最新のリビジョン番号を読み直せば保存できる
  expectSuccess(await yomi.saveDraftRaw(asSession(otherSession), a.id, e.id, { body: simpleBody(secondMarker) }, now.revision), "最新のリビジョン番号での保存");
});
