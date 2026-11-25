// フィールドの型の約束: INV-FLD-01・02（M5、2026-11-25。02-schema-and-delivery-api.md「フィールドの型」）
//
// 数値・真偽値・日付・日時・選択肢・グループのフィールドを持つテンプレートを、テストのために1つデプロイし、
// そのテンプレートのワークスペースで確かめる。保存の拒否は、記事の作成と下書きの保存の両方で、
// また保存前のフックが入れた値でも確かめる（フックの直しも保存の前に検証し、通らない直しは使わない）。

import { randomUUID } from "node:crypto";
import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asBearer, asSession, driver, type Session } from "./support/driver.js";
import { expectRefused, expectSuccess } from "./support/assert.js";
import { doc, paragraph } from "./support/body.js";
import { pngImage } from "./support/images.js";
import { beforeSaveSettingField } from "./support/scripts.js";
import { cloneTemplate, postType, type FieldSpec } from "./support/template.js";
import { lazyWorld, marker } from "./support/world.js";

const world = lazyWorld();

/** グループの中に置くフィールド。グループに入れられる型をひととおり持つ。 */
const FAQ_FIELDS: FieldSpec[] = [
  { name: "question", type: "text", required: true },
  { name: "answer", type: "text", required: true },
  { name: "position", type: "number", integer: true, min: 1, max: 10 },
  { name: "level", type: "select", options: ["basic", "advanced"] },
  { name: "open", type: "boolean" },
  { name: "asked_on", type: "date" },
  { name: "answered_at", type: "datetime" },
  { name: "figure", type: "image" },
];

const FIELDS: FieldSpec[] = [
  { name: "rating", type: "number", integer: true, min: 0, max: 100 },
  { name: "views", type: "number", integer: true },
  { name: "price", type: "number" },
  { name: "featured", type: "boolean" },
  { name: "event_on", type: "date" },
  { name: "starts_at", type: "datetime" },
  { name: "category", type: "select", options: ["news", "guide", "release"] },
  { name: "topics", type: "select", options: ["rust", "nuxt", "postgres"], multiple: true },
  { name: "faq", type: "group", fields: FAQ_FIELDS, maxItems: 5 },
];

interface Site {
  id: string;
  editorSession: Session;
  /** 別のワークスペースのメディア（グループの中の画像の検証に使う）。 */
  otherWorkspaceMediaId: string;
  /** このワークスペースのメディア。 */
  ownMediaId: string;
}

let sitePromise: Promise<Site> | null = null;
const site = (): Promise<Site> => (sitePromise ??= createSite());

async function createSite(): Promise<Site> {
  const { yomi, serverAdmin, serverAdminSession, b } = await world();
  const name = `inv-fld-${randomUUID().slice(0, 8)}`;
  const template = cloneTemplate(name, (t) => {
    const post = postType(t);
    post.fields.push(...structuredClone(FIELDS));
    post.hooks = { beforeSave: { source: beforeSaveSettingField(), permissions: [] } };
  });
  expectSuccess(await yomi.deployTemplateRaw(asBearer(await yomi.deployToken(serverAdminSession, serverAdmin)), template), "新しい型のフィールドを持つテンプレートのデプロイ");
  await yomi.reauthenticate(serverAdminSession, serverAdmin);
  const ws = await yomi.createWorkspace(serverAdminSession, `${name}-ws`, { template: name });
  const admin = await yomi.createMember(serverAdminSession, ws.id, "admin");
  const adminSession = await yomi.login(admin);
  const editorSession = await yomi.login(await yomi.createMember(adminSession, ws.id, "editor"));
  const png = () => ({ filename: "fld.png", bytes: pngImage(8, 8), contentType: "image/png" });
  const own = await yomi.uploadMedia(editorSession, ws.id, png());
  const other = await yomi.uploadMedia(b.editorSession, b.id, png());
  return { id: ws.id, editorSession, ownMediaId: own.id, otherWorkspaceMediaId: other.id };
}

/**
 * fields の値を、記事の作成・下書きの保存・保存前のフックの直しの3つの道で保存しようとし、どれでも値が入らないことを確かめる。
 * 作成と保存は拒否されること。フックの直しは、拒否されるか、使われずにその理由が知らされること。
 */
async function expectRefusedEverywhere(s: Site, field: string, value: unknown, label: string): Promise<void> {
  const { yomi } = await world();
  const session = s.editorSession;
  expectRefused(
    await yomi.createEntryRaw(asSession(session), s.id, { title: marker("fld-new"), body: doc(paragraph("x")), fields: { [field]: value } }),
    `${label}を入れた記事の作成`,
  );

  const target = await yomi.createEntry(session, s.id, { title: marker("fld"), body: doc(paragraph("x")) });
  const before = await yomi.getDraft(session, s.id, target.id);
  expectRefused(await yomi.saveDraftRaw(asSession(session), s.id, target.id, { fields: { [field]: value } }, before.revision), `${label}を入れた下書きの保存`);
  // フックの直しが検査に通らないときは、保存ごと拒否するか、直しを使わずに保存して理由を知らせる
  // （フックの不具合で編集者の自動保存を止めない。設計の原則10、2026-11-25 利用者の判断）。どちらでも値は入らない
  const hookBefore = await yomi.getDraft(session, s.id, target.id);
  const hooked = await yomi.saveDraftRaw(
    asSession(session),
    s.id,
    target.id,
    { fields: { summary: `set:${field}=${JSON.stringify(value)}` } },
    hookBefore.revision,
  );
  if (hooked.status < 400) {
    expectSuccess(hooked, `保存前のフックが${label}を入れる下書きの保存`);
    expect(yomi.hookFailureOf(hooked), `保存前のフックが入れた${label}が使われなかったことが知らされていない`).not.toBeNull();
  } else {
    expectRefused(hooked, `保存前のフックが${label}を入れる下書きの保存`);
  }
  const after = await yomi.getDraft(session, s.id, target.id);
  expect(after.fields[field] ?? null, `${label}が下書きに保存された`).toBeNull();
}

/** fields の値を下書きに保存でき、保存した値が読み出せることを確かめる（対照）。 */
async function expectSaved(s: Site, fields: Record<string, unknown>, label: string): Promise<string> {
  const { yomi } = await world();
  const session = s.editorSession;
  const target = await yomi.createEntry(session, s.id, { title: marker("fld-ok"), body: doc(paragraph("x")) });
  const before = await yomi.getDraft(session, s.id, target.id);
  expectSuccess(await yomi.saveDraftRaw(asSession(session), s.id, target.id, { fields }, before.revision), `${label}の下書きの保存`);
  const after = await yomi.getDraft(session, s.id, target.id);
  for (const [name, value] of Object.entries(fields)) {
    expect(after.fields[name], `${label}: ${name} の値が保存されていない`).toBeDefined();
    if (typeof value !== "string" || !name.endsWith("_at")) {
      expect(after.fields[name], `${label}: ${name} の値が保存のとおりでない`).toEqual(value);
    }
  }
  return target.id;
}

inv("inv_fld_01", "数値・真偽値・日付・日時・選択肢のフィールドに、型に合わない値（選択肢にない値を含む）を入れた下書きは、保存時に拒否される", async () => {
  const s = await site();

  for (const [field, value, label] of [
    // 整数（範囲あり: 0〜100）
    ["rating", "50", "整数のフィールドに文字列"],
    ["rating", 12.5, "整数のフィールドに小数"],
    ["rating", 101, "整数のフィールドに上限を超える値"],
    ["rating", -1, "整数のフィールドに下限を下回る値"],
    ["rating", true, "整数のフィールドに真偽値"],
    // 整数（範囲なし）: 32ビットに収まらない値は配信APIの Int で返せない
    ["views", 2147483648, "整数のフィールドに32ビットを超える値"],
    ["views", -2147483649, "整数のフィールドに32ビットを下回る値"],
    // 小数
    ["price", "980", "数値のフィールドに文字列"],
    ["price", { amount: 980 }, "数値のフィールドにオブジェクト"],
    ["price", [980], "数値のフィールドに配列"],
    // 真偽値
    ["featured", "true", "真偽値のフィールドに文字列"],
    ["featured", 1, "真偽値のフィールドに数値"],
    // 日付（YYYY-MM-DD）
    ["event_on", "2026-13-01", "日付のフィールドに存在しない月"],
    ["event_on", "2026-02-30", "日付のフィールドに存在しない日"],
    ["event_on", "2026/11/25", "日付のフィールドに別の書式"],
    ["event_on", "2026-11-25T10:00:00Z", "日付のフィールドに日時"],
    ["event_on", 20261125, "日付のフィールドに数値"],
    // 日時（時差の付いた ISO 8601）
    ["starts_at", "2026-11-25", "日時のフィールドに日付だけ"],
    ["starts_at", "2026-11-25T25:00:00Z", "日時のフィールドに存在しない時刻"],
    ["starts_at", "2026-11-25T10:00:00", "日時のフィールドに時差のない日時"],
    ["starts_at", "明日の朝", "日時のフィールドに日時でない文字列"],
    ["starts_at", 1764064800, "日時のフィールドに数値"],
    // 選択肢
    ["category", "opinion", "選択肢のフィールドに選択肢にない値"],
    ["category", "News", "選択肢のフィールドに大文字小文字の違う値"],
    ["category", "", "選択肢のフィールドに空の文字列"],
    ["category", ["news"], "1つを選ぶ選択肢のフィールドに配列"],
    ["category", 1, "選択肢のフィールドに数値"],
    // 複数を選べる選択肢
    ["topics", ["rust", "go"], "複数を選べる選択肢のフィールドに選択肢にない値を含む配列"],
    ["topics", "rust", "複数を選べる選択肢のフィールドに配列でない値"],
    ["topics", [1], "複数を選べる選択肢のフィールドに数値の配列"],
  ] as const) {
    await expectRefusedEverywhere(s, field, value, label);
  }

  // 対照: 型に合う値は保存できる
  await expectSaved(
    s,
    {
      rating: 100,
      views: 2147483647,
      price: 980.5,
      featured: false,
      event_on: "2026-02-28",
      starts_at: "2026-11-25T10:00:00+09:00",
      category: "news",
      topics: ["rust", "postgres"],
    },
    "型に合う値",
  );
  await expectSaved(s, { rating: 0, views: -2147483648, price: -1, featured: true }, "範囲の端の値");
}, 300_000);

inv("inv_fld_02", "グループのフィールドの中の値も、それぞれのフィールドの型に従って保存時に検証され、グループの中の必須のフィールドが空の記事は公開できない", async () => {
  const s = await site();
  const { yomi } = await world();
  const ok = { question: "質問", answer: "答え" };

  // 保存時の検証: グループの中の値も、それぞれの型に従う
  for (const [item, label] of [
    [{ ...ok, position: "1" }, "グループの中の整数のフィールドに文字列"],
    [{ ...ok, position: 11 }, "グループの中の整数のフィールドに上限を超える値"],
    [{ ...ok, position: 1.5 }, "グループの中の整数のフィールドに小数"],
    [{ ...ok, level: "expert" }, "グループの中の選択肢のフィールドに選択肢にない値"],
    [{ ...ok, open: "yes" }, "グループの中の真偽値のフィールドに文字列"],
    [{ ...ok, asked_on: "2026-02-30" }, "グループの中の日付のフィールドに存在しない日"],
    [{ ...ok, answered_at: "2026-11-25T10:00:00" }, "グループの中の日時のフィールドに時差のない日時"],
    [{ ...ok, question: 42 }, "グループの中の文字列のフィールドに数値"],
    [{ ...ok, figure: driver.fields.image({ mediaId: s.otherWorkspaceMediaId, alt: "図" }) }, "グループの中の画像のフィールドにほかのワークスペースのメディア"],
    [{ ...ok, figure: driver.fields.image({ mediaId: randomUUID(), alt: "図" }) }, "グループの中の画像のフィールドに存在しないメディア"],
  ] as const) {
    // 正しい件と並べても、1件でも型に合わなければ拒否される
    await expectRefusedEverywhere(s, "faq", [ok, item], label);
  }
  for (const [value, label] of [
    [ok, "グループのフィールドに配列でない値"],
    [["質問"], "グループのフィールドに、件がオブジェクトでない配列"],
  ] as const) {
    await expectRefusedEverywhere(s, "faq", value, label);
  }

  // 対照: 型に合う値は保存できる
  const full = {
    question: "質問",
    answer: "答え",
    position: 1,
    level: "advanced",
    open: true,
    asked_on: "2026-11-25",
    answered_at: "2026-11-25T10:00:00Z",
    figure: driver.fields.image({ mediaId: s.ownMediaId, alt: "図" }),
  };
  const saved = await yomi.createEntry(s.editorSession, s.id, { title: marker("fld02-ok"), body: doc(paragraph("x")), fields: { faq: [full, ok] } });
  const draft = await yomi.getDraft(s.editorSession, s.id, saved.id);
  const items = draft.fields["faq"] as Record<string, unknown>[] | undefined;
  expect(Array.isArray(items) && items.length === 2, "型に合うグループの値が保存されていない").toBe(true);
  expect(items![0]!["level"], "グループの中の値が保存のとおりでない").toBe("advanced");
  expect(items![0]!["position"], "グループの中の値が保存のとおりでない").toBe(1);

  // 公開: グループの中の必須のフィールドが空の記事は、下書きには保存できるが、公開できない
  for (const [faq, label] of [
    [[ok, { question: "質問" }], "2件目の必須のフィールドがない"],
    [[{ question: "質問", answer: "" }], "必須のフィールドが空の文字列"],
    [[{ question: null, answer: "答え" }], "必須のフィールドが null"],
  ] as const) {
    // 下書きの保存は通る（必須の検査は公開のとき。作成に失敗すればドライバーが例外を投げる）
    const e = await yomi.createEntry(s.editorSession, s.id, { title: marker("fld02-req"), body: doc(paragraph("x")), fields: { faq } });
    expectRefused(await yomi.publishRaw(asSession(s.editorSession), s.id, e.id), `グループの${label}記事の公開`);
  }

  // 対照: 必須のフィールドがそろっていれば公開でき、グループが空（0件）でも公開できる（グループ自体は必須でない）
  expectSuccess(await yomi.publishRaw(asSession(s.editorSession), s.id, saved.id), "グループの必須のフィールドがそろった記事の公開");
  const empty = await yomi.createEntry(s.editorSession, s.id, { title: marker("fld02-empty"), body: doc(paragraph("x")), fields: { faq: [] } });
  expectSuccess(await yomi.publishRaw(asSession(s.editorSession), s.id, empty.id), "グループが空の記事の公開");
}, 300_000);
