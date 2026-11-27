// 配信APIの一覧の絞り込みと並べ替えの約束: INV-FLD-03・04（M5、2026-11-26。02-schema-and-delivery-api.md「フィールドの型」）
//
// 絞り込める型: 選択肢・真偽値・参照（等しい・含む）、数値・日付・日時（範囲）。条件はすべてを満たすもの（AND）で、5つまで。
// 並べ替えのキーは1つ（数値・日付・日時・公開日時）。対象は直下のフィールドだけで、グループの中のフィールドは使えない。
//
// 新しい型のフィールドを持つテンプレートを1つデプロイし、そのテンプレートのワークスペースで確かめる。
// INV-FLD-03 では、公開中の版の値と違う値を、下書き・予約中の版・非公開や削除済みの記事・公開していない記事に入れておき、
// 絞り込みと並べ替えの結果がそれらに引きずられないことを確かめる。結果の記事が正しいことを対照として確かめ、
// 「何も返さない」実装では通らないようにする。

import { randomUUID } from "node:crypto";
import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asBearer, asSession, type IssuedSecret, type ListConditions, type ListFilter, type RegisteredQuery, type Session } from "./support/driver.js";
import { expectSuccess } from "./support/assert.js";
import { doc, paragraph } from "./support/body.js";
import { isSuccess } from "./support/http.js";
import { cloneTemplate, postType, type FieldSpec } from "./support/template.js";
import { lazyWorld, marker } from "./support/world.js";

const world = lazyWorld();

const FIELDS: FieldSpec[] = [
  { name: "category", type: "select", options: ["news", "guide", "release"] },
  { name: "topics", type: "select", options: ["rust", "nuxt", "postgres"], multiple: true },
  { name: "featured", type: "boolean" },
  { name: "rating", type: "number", integer: true },
  { name: "price", type: "number" },
  { name: "event_on", type: "date" },
  { name: "starts_at", type: "datetime" },
  { name: "author", type: "reference", referenceTo: "post" },
];

/** 一覧の件数の上限（このテストで作る記事がすべて1回で返る大きさ）。 */
const LIST_LIMIT = 50;

interface Site {
  id: string;
  editorSession: Session;
  readKey: IssuedSecret;
  registerKey: IssuedSecret;
  /** 条件を変数で渡す一覧のクエリ。 */
  list: RegisteredQuery;
}

let sitePromise: Promise<Site> | null = null;
const site = (): Promise<Site> => (sitePromise ??= createSite());

async function createSite(): Promise<Site> {
  const { yomi, serverAdmin, serverAdminSession } = await world();
  const name = `inv-fltr-${randomUUID().slice(0, 8)}`;
  const template = cloneTemplate(name, (t) => {
    const post = postType(t);
    post.listLimit = LIST_LIMIT;
    post.fields.push(...structuredClone(FIELDS));
  });
  expectSuccess(await yomi.deployTemplateRaw(asBearer(await yomi.deployToken(serverAdminSession, serverAdmin)), template), "絞り込める型のフィールドを持つテンプレートのデプロイ");
  await yomi.reauthenticate(serverAdminSession, serverAdmin);
  const { id } = await yomi.createWorkspace(serverAdminSession, `${name}-ws`, { template: name });
  const admin = await yomi.createMember(serverAdminSession, id, "admin");
  const adminSession = await yomi.login(admin);
  const editorSession = await yomi.login(await yomi.createMember(adminSession, id, "editor"));
  await yomi.reauthenticate(adminSession, admin);
  const readKey = await yomi.createApiKey(adminSession, id, ["read"]);
  const registerKey = await yomi.createApiKey(adminSession, id, ["register"]);
  const list = await yomi.registerQuery(id, registerKey.secret, { kind: "filteredList" });
  return { id, editorSession, readKey, registerKey, list };
}

/** 記事を作る（公開はしない）。 */
async function draft(s: Site, label: string, fields: Record<string, unknown> = {}): Promise<string> {
  const { yomi } = await world();
  const { id } = await yomi.createEntry(s.editorSession, s.id, { title: marker(label), body: doc(paragraph("x")), fields });
  return id;
}

/** 記事を作って公開する。 */
async function published(s: Site, label: string, fields: Record<string, unknown> = {}): Promise<string> {
  const { yomi } = await world();
  const id = await draft(s, label, fields);
  await yomi.publishEntry(s.editorSession, s.id, id);
  return id;
}

/** 条件で一覧を取り、記事の id を並びのとおりに返す。一覧が返らなければ失敗する。 */
async function listed(s: Site, conditions: ListConditions, what: string): Promise<string[]> {
  const { yomi } = await world();
  const res = await yomi.filteredListRaw(s.id, s.readKey.secret, s.list, { conditions });
  expectSuccess(res, what);
  const ids = yomi.listedIds(res);
  expect(ids, `${what}: 一覧が返らなかった`).not.toBeNull();
  return ids!;
}

/** 1つの条件で絞り込んだ結果が、expected の記事とちょうど一致する（順は問わない）ことを確かめる。 */
async function expectFiltered(s: Site, filter: ListFilter, expected: string[], what: string): Promise<void> {
  const ids = await listed(s, { filters: [filter] }, what);
  expect([...ids].sort(), what).toEqual([...expected].sort());
}

inv("inv_fld_03", "配信APIの一覧の絞り込みと並べ替えには、公開中の版の値だけが使われる。下書きや予約中の版の値、非公開・削除済みの記事は結果に影響せず、参照のフィールドでの絞り込みは、参照先が公開中の記事のときだけ一致する", async () => {
  const s = await site();
  const { yomi, b } = await world();

  // 公開中の版の値（published）と、下書き・予約版に入れる違う値（other）
  const published1 = {
    category: "news",
    topics: ["rust"],
    featured: true,
    rating: 10,
    price: 1.5,
    event_on: "2026-01-10",
    starts_at: "2026-01-10T00:00:00Z",
  };
  const other = {
    category: "release",
    topics: ["postgres"],
    featured: false,
    rating: 95,
    price: 99.5,
    event_on: "2026-12-10",
    starts_at: "2026-12-10T00:00:00Z",
  };

  // P: 公開した後、下書きだけを other に変えた記事
  const p = await published(s, "fltr-draft", published1);
  await yomi.saveDraft(s.editorSession, s.id, p, { fields: other });

  // S: 公開した後、other の版を先の日時に予約した記事
  const sched = await published(s, "fltr-sched", { ...published1, rating: 20, price: 2, featured: false, topics: ["nuxt"] });
  await yomi.saveDraft(s.editorSession, s.id, sched, { fields: other });
  await yomi.schedulePublish(s.editorSession, s.id, sched, new Date(Date.now() + 7 * 24 * 3_600_000));

  // Q: 対照の公開中の記事
  const q = await published(s, "fltr-control", {
    category: "guide",
    topics: ["nuxt", "postgres"],
    featured: false,
    rating: 5,
    price: 3,
    event_on: "2026-06-01",
    starts_at: "2026-06-01T12:00:00Z",
  });

  // 非公開・削除済み・公開していない記事。値は P の公開中の版と同じ（絞り込みに一致する値）
  const unpublished = await published(s, "fltr-unpub", { ...published1, rating: 15 });
  await yomi.unpublishEntry(s.editorSession, s.id, unpublished);
  const deleted = await published(s, "fltr-deleted", { ...published1, rating: 12 });
  await yomi.deleteEntry(s.editorSession, s.id, deleted);
  const neverPublished = await draft(s, "fltr-never", { ...published1, rating: 8 });

  // --- 絞り込み: 公開中の版の値で一致し、ほかの値では一致しない ---
  await expectFiltered(s, { field: "category", equals: "news" }, [p, sched], "選択肢が news の一覧");
  await expectFiltered(s, { field: "category", equals: "guide" }, [q], "選択肢が guide の一覧");
  await expectFiltered(s, { field: "category", equals: "release" }, [], "下書き・予約版にだけある選択肢の一覧");
  await expectFiltered(s, { field: "topics", contains: "rust" }, [p], "複数の選択肢に rust を含む一覧");
  await expectFiltered(s, { field: "topics", contains: "postgres" }, [q], "下書き・予約版にもある選択肢を含む一覧");
  await expectFiltered(s, { field: "featured", equals: true }, [p], "真偽値が true の一覧");
  await expectFiltered(s, { field: "featured", equals: false }, [sched, q], "真偽値が false の一覧");
  await expectFiltered(s, { field: "rating", atLeast: 0, atMost: 25 }, [p, sched, q], "整数の範囲の一覧");
  await expectFiltered(s, { field: "rating", atLeast: 90 }, [], "下書き・予約版にだけある整数の範囲の一覧");
  await expectFiltered(s, { field: "price", atLeast: 50 }, [], "下書き・予約版にだけある小数の範囲の一覧");
  await expectFiltered(s, { field: "event_on", atMost: "2026-02-01" }, [p, sched], "日付の範囲の一覧");
  await expectFiltered(s, { field: "event_on", atLeast: "2026-12-01" }, [], "下書き・予約版にだけある日付の範囲の一覧");
  await expectFiltered(s, { field: "starts_at", atMost: "2026-02-01T00:00:00Z" }, [p, sched], "日時の範囲の一覧");
  await expectFiltered(s, { field: "starts_at", atLeast: "2026-12-01T00:00:00Z" }, [], "下書き・予約版にだけある日時の範囲の一覧");

  // --- 並べ替え: 公開中の版の値の順 ---
  const rated = { field: "rating", atLeast: 0, atMost: 100 } as const;
  expect(await listed(s, { filters: [rated], sort: [{ field: "rating", direction: "desc" }] }, "整数の降順の一覧"), "整数の降順").toEqual([sched, p, q]);
  expect(await listed(s, { filters: [rated], sort: [{ field: "rating", direction: "asc" }] }, "整数の昇順の一覧"), "整数の昇順").toEqual([q, p, sched]);
  expect(await listed(s, { filters: [rated], sort: [{ field: "price", direction: "desc" }] }, "小数の降順の一覧"), "小数の降順").toEqual([q, sched, p]);

  // 非公開・削除済み・公開していない記事は、絞り込みも並べ替えもしない一覧にも出ない
  const all = await listed(s, {}, "条件のない一覧");
  for (const [id, label] of [
    [unpublished, "非公開にした記事"],
    [deleted, "削除した記事"],
    [neverPublished, "公開していない記事"],
  ] as const) {
    expect(all, `${label}が一覧に出た`).not.toContain(id);
  }

  // --- 参照: 参照先が公開中の記事のときだけ一致する ---
  const groupOf = (entryId: string) => yomi.translationGroupId(s.editorSession, s.id, entryId);
  const live = await published(s, "fltr-ref-live");
  const hidden = await published(s, "fltr-ref-hidden");
  const removed = await published(s, "fltr-ref-removed");
  const unreleased = await draft(s, "fltr-ref-unreleased");
  const elsewhere = (await yomi.createEntry(b.editorSession, b.id, { title: marker("fltr-ref-other-ws"), body: doc(paragraph("x")) })).id;
  await yomi.publishEntry(b.editorSession, b.id, elsewhere);
  const groups = {
    live: await groupOf(live),
    hidden: await groupOf(hidden),
    removed: await groupOf(removed),
    unreleased: await groupOf(unreleased),
    elsewhere: await yomi.translationGroupId(b.editorSession, b.id, elsewhere),
  };

  // 参照する記事は、参照先がすべて公開中のうちに公開しておく
  const byLive = await published(s, "fltr-by-live", { author: groups.live });
  const byHidden = await published(s, "fltr-by-hidden", { author: groups.hidden });
  const byRemoved = await published(s, "fltr-by-removed", { author: groups.removed });
  await published(s, "fltr-by-unreleased", { author: groups.unreleased });
  // ほかのワークスペースの記事への参照は、保存の時点で拒否されてもよい（拒否されれば、一致しうる記事がない）。
  // 保存できたなら公開し、下の絞り込みで一致しないことを確かめる
  const byElsewhere = await draft(s, "fltr-by-elsewhere");
  const before = await yomi.getDraft(s.editorSession, s.id, byElsewhere);
  if (isSuccess(await yomi.saveDraftRaw(asSession(s.editorSession), s.id, byElsewhere, { fields: { author: groups.elsewhere } }, before.revision))) {
    await yomi.publishEntry(s.editorSession, s.id, byElsewhere);
  }
  const relatedBoth = await published(s, "fltr-rel-both", { related: [groups.hidden, groups.live] });
  await published(s, "fltr-rel-hidden", { related: [groups.hidden, groups.unreleased] });
  await yomi.unpublishEntry(s.editorSession, s.id, hidden);
  await yomi.deleteEntry(s.editorSession, s.id, removed);

  await expectFiltered(s, { field: "author", equals: groups.live }, [byLive], "公開中の記事を参照する一覧");
  await expectFiltered(s, { field: "related", contains: groups.live }, [relatedBoth], "公開中の記事を参照に含む一覧");
  for (const [group, label] of [
    [groups.hidden, "非公開にした記事"],
    [groups.removed, "削除した記事"],
    [groups.unreleased, "公開していない記事"],
    [groups.elsewhere, "ほかのワークスペースの公開中の記事"],
  ] as const) {
    await expectFiltered(s, { field: "author", equals: group }, [], `${label}を参照する一覧`);
    await expectFiltered(s, { field: "related", contains: group }, [], `${label}を参照に含む一覧`);
  }
  // 参照する側の記事は公開中のまま（参照先が見えなくなっても、条件のない一覧には出る）
  const after = await listed(s, {}, "参照先を非公開・削除した後の条件のない一覧");
  expect(after, "参照先を非公開にした記事が一覧から消えた").toContain(byHidden);
  expect(after, "参照先を削除した記事が一覧から消えた").toContain(byRemoved);
}, 300_000);

inv("inv_fld_04", "配信APIの一覧で、絞り込みの条件が5つを超えるリクエストと、並べ替えのキーを2つ以上指定したリクエストは拒否される。条件をクエリに書き込んで登録した場合も、変数で渡した場合も同じ", async () => {
  const s = await site();
  const { yomi } = await world();
  const five: ListFilter[] = [
    { field: "category", equals: "news" },
    { field: "featured", equals: true },
    { field: "rating", atLeast: 0 },
    { field: "price", atLeast: 0 },
    { field: "event_on", atLeast: "2000-01-01" },
  ];
  const six: ListFilter[] = [...five, { field: "starts_at", atLeast: "2000-01-01T00:00:00Z" }];
  const oneKey = [{ field: "rating", direction: "desc" }] as const;
  const twoKeys = [
    { field: "rating", direction: "desc" },
    { field: "price", direction: "asc" },
  ] as const;

  // 変数で渡す
  const viaVariables = (conditions: ListConditions) => yomi.filteredListRaw(s.id, s.readKey.secret, s.list, { conditions });
  expect(yomi.listedIds(await viaVariables({ filters: five, sort: [...oneKey] })), "条件5つ・キー1つの一覧（変数）が返らない").not.toBeNull();
  expect(yomi.listedIds(await viaVariables({ filters: six })), "条件6つの一覧（変数）が返った").toBeNull();
  expect(yomi.listedIds(await viaVariables({ sort: [...twoKeys] })), "並べ替えのキー2つの一覧（変数）が返った").toBeNull();

  // クエリに書き込んで登録する: 登録で拒否されるか、登録できても実行で拒否される
  const viaFixedQuery = async (fixed: ListConditions): Promise<string[] | null> => {
    const spec = { kind: "filteredList", fixed } as const;
    if (!isSuccess(await yomi.registerQueryRaw(s.id, s.registerKey.secret, spec))) return null;
    // 同じ文の登録は同じクエリになる（graphql.test.ts の inv_gql_04 と同じ）
    const query = await yomi.registerQuery(s.id, s.registerKey.secret, spec);
    return yomi.listedIds(await yomi.filteredListRaw(s.id, s.readKey.secret, query));
  };
  expect(await viaFixedQuery({ filters: five, sort: [...oneKey] }), "条件5つ・キー1つを書き込んだ一覧が返らない").not.toBeNull();
  expect(await viaFixedQuery({ filters: six }), "条件6つを書き込んだ一覧が返った").toBeNull();
  expect(await viaFixedQuery({ sort: [...twoKeys] }), "並べ替えのキー2つを書き込んだ一覧が返った").toBeNull();
}, 300_000);
