// 循環しない参照の約束: INV-SCH-11（M5、2026-11-28。02-schema-and-delivery-api.md「循環しない参照」）
//
// `acyclic` を付けた参照（自分のコンテンツタイプを指す単数の参照。docs の親ページ）では、公開中の記事の親をたどって
// 自分に戻らず、親の数は10以下に保たれる。親として数えるのは、同じロケールの公開中の記事だけ。
//
// 確かめること:
// - 通常の公開・訂正として公開で、自分に戻る版と、親が10を超える版は公開されない。途中の記事を別の親の下に移して、
//   その下の記事の親が10を超える場合も同じ（移した記事の公開で止まる）
// - 予約公開: 入れたときに破っていれば受け付けない。入れたときには守っていても、実行までに親が変わって破るなら公開されない
// - 破るデータがあるときに、既存の参照に `acyclic` を後から付けるデプロイは適用されない
//
// どれも、守る版の公開・予約・デプロイが成功することを対照として確かめ、「何でも拒否する」実装では通らないようにする。

import { randomUUID } from "node:crypto";
import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asBearer, asSession, type RegisteredQuery, type Session } from "./support/driver.js";
import { expectRefused, expectSuccess } from "./support/assert.js";
import { doc, paragraph } from "./support/body.js";
import { cloneTemplate, postType, type TemplateSpec } from "./support/template.js";
import { lazyWorld, marker, sleep } from "./support/world.js";

const world = lazyWorld();

/** 親の数の上限（02-schema-and-delivery-api.md「循環しない参照」）。 */
const MAX_PARENTS = 10;
/** 予約の時刻を過ぎてから、対照の予約が公開されるまで待つ上限。 */
const PUBLISH_TIMEOUT_MS = 30_000;
const soon = (seconds = 5) => new Date(Date.now() + seconds * 1000);

/** post に、自分のタイプを指す単数の参照 parent を足したテンプレート。 */
function pageTemplate(name: string, acyclic: boolean): TemplateSpec {
  return cloneTemplate(name, (t) => {
    postType(t).fields.push({ name: "parent", type: "reference", referenceTo: "post", ...(acyclic ? { acyclic: true } : {}) });
  });
}

interface Site {
  id: string;
  editorSession: Session;
  readKey: string;
  byId: RegisteredQuery;
}

/** テンプレートをデプロイし、それを使うワークスペースを作る。 */
async function createSite(template: TemplateSpec): Promise<Site> {
  const { yomi, serverAdmin, serverAdminSession } = await world();
  expectSuccess(await yomi.deployTemplateRaw(asBearer(await yomi.deployToken(serverAdminSession, serverAdmin)), template), `テンプレート ${template.name} のデプロイ`);
  await yomi.reauthenticate(serverAdminSession, serverAdmin);
  const { id } = await yomi.createWorkspace(serverAdminSession, `${template.name}-ws`, { template: template.name });
  const admin = await yomi.createMember(serverAdminSession, id, "admin");
  const adminSession = await yomi.login(admin);
  const editorSession = await yomi.login(await yomi.createMember(adminSession, id, "editor"));
  await yomi.reauthenticate(adminSession, admin);
  const readKey = await yomi.createApiKey(adminSession, id, ["read"]);
  const registerKey = await yomi.createApiKey(adminSession, id, ["register"]);
  const byId = await yomi.registerQuery(id, registerKey.secret, { kind: "byId" });
  return { id, editorSession, readKey: readKey.secret, byId };
}

/** 記事（ページ）。parent に入れるのは翻訳グループの ID。 */
interface Page {
  id: string;
  group: string;
  title: string;
}

/** 記事を作って公開する（parent があれば、その記事を親にする）。公開できなければ失敗する。 */
async function publishedPage(s: Site, label: string, parent?: Page): Promise<Page> {
  const { yomi } = await world();
  const title = marker(label);
  const { id } = await yomi.createEntry(s.editorSession, s.id, { title, body: doc(paragraph("x")), fields: parent ? { parent: parent.group } : {} });
  expectSuccess(await yomi.publishRaw(asSession(s.editorSession), s.id, id), `記事 ${label} の公開`);
  return { id, group: await yomi.translationGroupId(s.editorSession, s.id, id), title };
}

/** 根から順に親子をつないだ記事を、count 件作って公開する（最後の記事の親の数は count - 1）。 */
async function publishedChain(s: Site, label: string, count: number, root?: Page): Promise<Page[]> {
  const chain: Page[] = [];
  for (let i = 0; i < count; i++) {
    chain.push(await publishedPage(s, `${label}-${i}`, chain[i - 1] ?? root));
  }
  return chain;
}

/** 下書きの親を付け替える（title も変えて、公開されたかを配信で見分けられるようにする）。 */
async function reparent(s: Site, page: Page, parent: Page, title: string): Promise<void> {
  const { yomi } = await world();
  await yomi.saveDraft(s.editorSession, s.id, page.id, { title, fields: { parent: parent.group } });
}

/** 配信APIで読んだ、記事の公開中の版のタイトル（公開中でなければ null）。 */
async function deliveredTitle(s: Site, entryId: string): Promise<string | null> {
  const { yomi } = await world();
  const res = await yomi.deliveryRaw(s.id, s.readKey, s.byId, { id: entryId, locale: "ja" });
  expectSuccess(res, "配信APIでの記事の取得");
  const data = (JSON.parse(res.text) as { data?: Record<string, { title?: string } | null> }).data ?? {};
  return Object.values(data)[0]?.title ?? null;
}

async function waitForTitle(s: Site, entryId: string, title: string): Promise<boolean> {
  const until = Date.now() + PUBLISH_TIMEOUT_MS;
  while (Date.now() < until) {
    if ((await deliveredTitle(s, entryId)) === title) return true;
    await sleep(0.5);
  }
  return false;
}

inv("inv_sch_11", "`acyclic` を付けた参照のフィールドでは、公開中の記事の親をたどって自分に戻ることはなく、親の数は10以下に保たれる。通常の公開・訂正として公開・予約公開の実行のどれでも、これを破る版は公開されず、これを破るデータがあるときに `acyclic` を後から付けるデプロイは適用されない", async () => {
  const { yomi, serverAdmin, serverAdminSession } = await world();
  const prefix = `inv-acyc-${randomUUID().slice(0, 8)}`;
  const s = await createSite(pageTemplate(`${prefix}-a`, true));
  const editor = asSession(s.editorSession);

  // --- 循環: 自分を親にする版と、親をたどって自分に戻る版は、通常の公開でも訂正として公開でも公開されない
  const self = await publishedPage(s, "sch11-self");
  await reparent(s, self, self, marker("sch11-self-v2"));
  expectRefused(await yomi.publishRaw(editor, s.id, self.id), "自分を親にした版の公開");

  const a = await publishedPage(s, "sch11-a");
  const b = await publishedPage(s, "sch11-b", a); // 対照: 循環しない親は公開できる（publishedPage が成功を確かめる）
  await reparent(s, a, b, marker("sch11-a-v2"));
  expectRefused(await yomi.publishRaw(editor, s.id, a.id), "親をたどると自分に戻る版の公開");
  expectRefused(await yomi.publishRaw(editor, s.id, a.id, { correction: true, note: "訂正" }), "親をたどると自分に戻る版の、訂正としての公開");
  expect(await deliveredTitle(s, a.id), "拒否されたはずの循環する版が公開されている").toBe(a.title);

  // --- 深さ: 親の数が10の記事は公開でき、11になる版は公開されない
  const chain = await publishedChain(s, "sch11-deep", MAX_PARENTS + 1); // 最後の記事の親の数は10（対照）
  const tooDeep = await yomi.createEntry(s.editorSession, s.id, { title: marker("sch11-deep-11"), body: doc(paragraph("x")), fields: { parent: chain[MAX_PARENTS]!.group } });
  expectRefused(await yomi.publishRaw(editor, s.id, tooDeep.id), "親の数が11になる版の公開");

  // --- 深さ（下の記事）: 子孫を持つ記事を別の親の下に移して、子孫の親の数が10を超えるなら、移す版は公開されない
  const moved = await publishedChain(s, "sch11-moved", 6); // moved[5] の親の数は5
  const host = await publishedChain(s, "sch11-host", 6); // host[5] の親の数は5
  // host[5] の下に移すと moved[5] の親の数は 6 + 5 = 11
  await reparent(s, moved[0]!, host[5]!, marker("sch11-moved-0-deep"));
  expectRefused(await yomi.publishRaw(editor, s.id, moved[0]!.id), "子孫の親の数が11になる、親の付け替えの公開");
  // 対照: host[4] の下なら moved[5] の親の数は 5 + 5 = 10 で、公開できる
  const fits = marker("sch11-moved-0-fits");
  await reparent(s, moved[0]!, host[4]!, fits);
  expectSuccess(await yomi.publishRaw(editor, s.id, moved[0]!.id), "子孫の親の数が10になる、親の付け替えの公開");
  expect(await deliveredTitle(s, moved[0]!.id), "公開できた付け替えの版が配信されていない").toBe(fits);

  // --- 予約公開: 入れたときに循環する予約は受け付けない
  const c = await publishedPage(s, "sch11-c");
  const d = await publishedPage(s, "sch11-d", c);
  await reparent(s, c, d, marker("sch11-c-v2"));
  expectRefused(await yomi.scheduleRaw(editor, s.id, c.id, soon()), "親をたどると自分に戻る版の予約");

  // --- 予約公開: 入れたときは循環しなくても、実行までに親が変わって循環するなら公開されない
  const e = await publishedPage(s, "sch11-e");
  const f = await publishedPage(s, "sch11-f");
  const at = soon();
  const eScheduled = marker("sch11-e-scheduled");
  await reparent(s, e, f, eScheduled);
  expectSuccess(await yomi.scheduleRaw(editor, s.id, e.id, at), "入れたときには循環しない予約"); // e の親を f にする
  // 予約の実行の前に、f の親を e にして公開する（公開中の e の親はまだないので、この時点では循環しない）
  const fNext = marker("sch11-f-v2");
  await reparent(s, f, e, fNext);
  expectSuccess(await yomi.publishRaw(editor, s.id, f.id), "予約の実行の前の、f の親の付け替えの公開");
  // 対照: 同じ時刻に予約した、循環しない記事は公開される（予約の仕組みが動いたことを確かめてから、公開されないことを確かめる）
  const control = await publishedPage(s, "sch11-control");
  const controlNext = marker("sch11-control-v2");
  await yomi.saveDraft(s.editorSession, s.id, control.id, { title: controlNext });
  expectSuccess(await yomi.scheduleRaw(editor, s.id, control.id, at), "対照の予約");
  expect(await waitForTitle(s, control.id, controlNext), "対照の記事の予約が公開されない").toBe(true);
  expect(await deliveredTitle(s, e.id), "実行のときに循環する予約版が公開されている").toBe(e.title);
  expect(await deliveredTitle(s, f.id), "f の親の付け替えが配信されていない").toBe(fNext);

  // --- デプロイ: 公開中の記事が循環しているときに、`acyclic` を後から付けるデプロイは適用されない
  const deployAuth = async () => asBearer(await yomi.deployToken(serverAdminSession, serverAdmin));
  const loose = await createSite(pageTemplate(`${prefix}-cycle`, false));
  const g = await publishedPage(loose, "sch11-g");
  const h = await publishedPage(loose, "sch11-h", g);
  await reparent(loose, g, h, marker("sch11-g-v2"));
  expectSuccess(await yomi.publishRaw(asSession(loose.editorSession), loose.id, g.id), "`acyclic` のない参照での循環する版の公開");
  expectRefused(
    await yomi.deployTemplateRaw(await deployAuth(), pageTemplate(`${prefix}-cycle`, true), { allowDestructive: true }),
    "循環する公開中の記事があるときの、`acyclic` を後から付けるデプロイ",
  );
  // 適用されていなければ、このワークスペースでは今も自分を親にした版を公開できる
  const looseSelf = await publishedPage(loose, "sch11-loose-self");
  await reparent(loose, looseSelf, looseSelf, marker("sch11-loose-self-v2"));
  expectSuccess(await yomi.publishRaw(asSession(loose.editorSession), loose.id, looseSelf.id), "拒否されたデプロイの後の、自分を親にした版の公開");

  // --- デプロイ: 親の数が10を超える公開中の記事があるときも、`acyclic` を後から付けるデプロイは適用されない
  const deep = await createSite(pageTemplate(`${prefix}-deep`, false));
  await publishedChain(deep, "sch11-loose-deep", MAX_PARENTS + 2); // 最後の記事の親の数は11
  expectRefused(
    await yomi.deployTemplateRaw(await deployAuth(), pageTemplate(`${prefix}-deep`, true), { allowDestructive: true }),
    "親の数が11の公開中の記事があるときの、`acyclic` を後から付けるデプロイ",
  );

  // --- 対照: 破るデータがなければ、`acyclic` を後から付けるデプロイは適用され、その後は循環する版を公開できない
  const clean = await createSite(pageTemplate(`${prefix}-clean`, false));
  const i = await publishedPage(clean, "sch11-i");
  await publishedPage(clean, "sch11-j", i);
  expectSuccess(
    await yomi.deployTemplateRaw(await deployAuth(), pageTemplate(`${prefix}-clean`, true), { allowDestructive: true }),
    "破るデータがないときの、`acyclic` を後から付けるデプロイ",
  );
  const cleanSelf = await publishedPage(clean, "sch11-clean-self");
  await reparent(clean, cleanSelf, cleanSelf, marker("sch11-clean-self-v2"));
  expectRefused(await yomi.publishRaw(asSession(clean.editorSession), clean.id, cleanSelf.id), "`acyclic` を付けた後の、自分を親にした版の公開");
}, 600_000);
