// AI向け配信の約束: INV-AIX-01, INV-AIX-02, INV-AIX-03
//
// AI向けの素材は、記事のMarkdown版（配信APIの記事の aiMarkdown）と、記事の目録（配信APIの aiCatalog）。
// Markdown版に載せるものは、コンテンツタイプの aiMarkdown（見出し・メタデータ・本文・関係）で開発者が決める（07-ai-delivery.md）。
// 漏れてはいけない値は一意な目印で見分け、載るべき値が載っていることも対照として確かめる。
// テンプレートのデプロイはテンプレートを使うすべてのワークスペースに効くため、テストごとに別の名前のテンプレートを使う。

import { randomUUID } from "node:crypto";
import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asBearer, type IssuedSecret, type Session } from "./support/driver.js";
import { expectNotLeaked, expectSuccess } from "./support/assert.js";
import { doc, paragraph } from "./support/body.js";
import type { HttpResponse } from "./support/http.js";
import { DEVNOTE_MARKER, cloneTemplate, postType, type TemplateSpec } from "./support/template.js";
import { lazyWorld, marker } from "./support/world.js";

const world = lazyWorld();
const templateName = (label: string) => `inv-${label}-${randomUUID().slice(0, 8)}`;
/** URLに入るスラッグ（目印として使う。小文字・数字・ハイフン）。 */
const uniqueSlug = (label: string) => `${label}-${randomUUID().slice(0, 8)}`;

interface Site {
  id: string;
  adminSession: Session;
  editorSession: Session;
  readKey: IssuedSecret;
  registerKey: IssuedSecret;
}

/** post の aiMarkdown を指定したテンプレートをデプロイし、それを使うワークスペースを作る。 */
async function siteWith(label: string, aiMarkdown: NonNullable<TemplateSpec["contentTypes"][number]["aiMarkdown"]>): Promise<Site> {
  const { cms, serverAdmin, serverAdminSession } = await world();
  const name = templateName(label);
  const template = cloneTemplate(name, (t) => {
    postType(t).aiMarkdown = aiMarkdown;
  });
  expectSuccess(await cms.deployTemplateRaw(asBearer(await cms.deployToken(serverAdminSession, serverAdmin)), template), "aiMarkdown を指定したテンプレートのデプロイ");
  await cms.reauthenticate(serverAdminSession, serverAdmin);
  const { id } = await cms.createWorkspace(serverAdminSession, `${name}-ws`, { template: name });
  const admin = await cms.createMember(serverAdminSession, id, "admin");
  const adminSession = await cms.login(admin);
  const editor = await cms.createMember(adminSession, id, "editor");
  const editorSession = await cms.login(editor);
  await cms.reauthenticate(adminSession, admin);
  const readKey = await cms.createApiKey(adminSession, id, ["read"]);
  const registerKey = await cms.createApiKey(adminSession, id, ["register"]);
  return { id, adminSession, editorSession, readKey, registerKey };
}

/** 記事を作って公開する。 */
async function publish(site: Site, input: { title: string; slug?: string; body: string; fields?: Record<string, unknown> }): Promise<string> {
  const { cms } = await world();
  const { id } = await cms.createEntry(site.editorSession, site.id, { title: input.title, slug: input.slug, body: doc(paragraph(input.body)), fields: input.fields });
  await cms.publishEntry(site.editorSession, site.id, id);
  return id;
}

/** 記事の翻訳グループのID（参照フィールドの値）。 */
async function groupOf(site: Site, entryId: string): Promise<string> {
  const { cms } = await world();
  return (await cms.getTranslationInfo(site.editorSession, site.id, entryId)).groupId;
}

/** 記事のMarkdown版（配信APIの応答のまま）。記事そのものの項目は id だけを取る。 */
async function markdownOf(site: Site, entryId: string): Promise<HttpResponse> {
  const { cms } = await world();
  const query = await cms.registerQuery(site.id, site.registerKey.secret, { kind: "byId", fields: ["id"], include: ["aiMarkdown"] });
  const res = await cms.deliveryRaw(site.id, site.readKey.secret, query, { id: entryId, locale: "ja" });
  expectSuccess(res, "配信APIでのMarkdown版の取得");
  return res;
}

/** 公開URLで取る記事のMarkdown版（配信APIの応答のまま。連携部品の `.md` が使う）。 */
async function markdownByUrl(site: Site, entryId: string): Promise<HttpResponse> {
  const { cms } = await world();
  const byId = await cms.registerQuery(site.id, site.registerKey.secret, { kind: "byId", fields: ["id", "url"] });
  const entry = await cms.deliveryRaw(site.id, site.readKey.secret, byId, { id: entryId, locale: "ja" });
  expectSuccess(entry, "配信APIでの記事のURLの取得");
  const url = (JSON.parse(entry.text) as { data: Record<string, { url: string } | null> }).data;
  const query = await cms.registerQuery(site.id, site.registerKey.secret, { kind: "aiMarkdownByUrl" });
  const res = await cms.deliveryRaw(site.id, site.readKey.secret, query, { url: Object.values(url)[0]!.url });
  expectSuccess(res, "配信APIでの公開URLによるMarkdown版の取得");
  return res;
}

/** 記事の目録（配信APIの応答のまま）。 */
async function catalogOf(site: Site): Promise<HttpResponse> {
  const { cms } = await world();
  const query = await cms.registerQuery(site.id, site.registerKey.secret, { kind: "aiCatalog" });
  const res = await cms.deliveryRaw(site.id, site.readKey.secret, query, { locale: "ja", limit: 1000 });
  expectSuccess(res, "配信APIでの目録の取得");
  return res;
}

const has = (res: HttpResponse, value: string) => res.text.includes(value);

inv("inv_aix_01", "「外部AI向けの素材に含める」がOFFの記事は、Markdown版・目録・他の記事の関係の相手のいずれにも含まれない", async () => {
  const { cms } = await world();
  const site = await siteWith("aix01", { title: "title", fields: ["body"], relations: ["related"] });
  // 関係の相手（後でOFFにする記事）と、それを関係に持つ記事。相手は、URLに入るスラッグとタイトルの目印で見分ける
  const targetTitle = marker("aix01-target");
  const targetSlug = uniqueSlug("aix01-target");
  const target = await publish(site, { title: targetTitle, slug: targetSlug, body: `本文 ${marker("aix01-target-body")}` });
  const source = await publish(site, { title: marker("aix01-source"), body: "関係を持つ記事", fields: { related: [await groupOf(site, target)] } });

  // 対照: 「含める」（初期値）のあいだは、どれにも含まれる
  expect(has(await markdownOf(site, target), targetTitle), "「含める」の記事のMarkdown版に、記事のタイトルがない").toBe(true);
  expect(has(await markdownByUrl(site, target), targetTitle), "公開URLで取った「含める」の記事のMarkdown版に、記事のタイトルがない").toBe(true);
  expect(has(await catalogOf(site), targetSlug), "「含める」の記事が目録にない").toBe(true);
  expect(has(await markdownOf(site, source), targetSlug), "関係の相手（「含める」の記事）のURLが、もう一方の記事のMarkdown版にない").toBe(true);

  // OFFにしたら、保存済みの素材を信用せず、すぐにどこからも消える（原則3）。方針のスイッチは、既定では管理者だけが変えられる
  await cms.setIncludeInAiMaterials(site.adminSession, site.id, target, false);
  for (const value of [targetTitle, targetSlug]) {
    expectNotLeaked(await markdownOf(site, target), value, "OFFの記事のMarkdown版");
    expectNotLeaked(await markdownByUrl(site, target), value, "公開URLで取ったOFFの記事のMarkdown版");
    expectNotLeaked(await catalogOf(site), value, "目録（OFFの記事）");
    expectNotLeaked(await markdownOf(site, source), value, "OFFの記事を関係に持つ記事のMarkdown版");
  }
  // 関係を持つ記事のほうは、引き続き素材になる
  expect(has(await catalogOf(site), source), "OFFの記事を関係に持つ記事まで、目録から消えた").toBe(true);
});

inv("inv_aix_02", "記事のMarkdown版には、スキーマでMarkdown版に含めると指定したフィールドだけが含まれる", async () => {
  // 同じ記事の内容を、Markdown版に載せるフィールドの指定が違う2つのテンプレートで確かめる
  const cases: { label: string; aiMarkdown: NonNullable<TemplateSpec["contentTypes"][number]["aiMarkdown"]>; listed: string[]; unlisted: string[] }[] = [
    { label: "aix02a", aiMarkdown: { title: "title", meta: ["summary"], fields: ["body"] }, listed: ["title", "summary", "body"], unlisted: ["subtitle"] },
    { label: "aix02b", aiMarkdown: { title: "title", fields: ["subtitle"] }, listed: ["title", "subtitle"], unlisted: ["summary", "body"] },
  ];
  for (const c of cases) {
    const site = await siteWith(c.label, c.aiMarkdown);
    const values: Record<string, string> = {
      title: marker(`${c.label}-title`),
      subtitle: marker(`${c.label}-subtitle`),
      summary: marker(`${c.label}-summary`),
      body: marker(`${c.label}-body`),
    };
    const id = await publish(site, { title: values["title"]!, body: values["body"]!, fields: { subtitle: values["subtitle"], summary: values["summary"] } });
    const markdown = await markdownOf(site, id);
    const catalog = await catalogOf(site);
    // 対照: 指定したフィールドは載っている
    for (const f of c.listed) expect(has(markdown, values[f]!), `${c.label}: Markdown版に、指定したフィールド ${f} がない`).toBe(true);
    for (const f of c.unlisted) {
      expectNotLeaked(markdown, values[f]!, `${c.label}: Markdown版（指定していないフィールド ${f}）`);
      expectNotLeaked(catalog, values[f]!, `${c.label}: 目録（指定していないフィールド ${f}）`);
    }
  }
});

inv("inv_aix_03", "`devNote` の内容は、AI向け素材に含まれない", async () => {
  // devNote はコンテンツタイプと summary フィールドに付いている（support/template.ts）。summary は Markdown版に載せる
  const site = await siteWith("aix03", { title: "title", meta: ["summary"], fields: ["summary", "body"], relations: ["related"] });
  const summary = marker("aix03-summary");
  const target = await publish(site, { title: marker("aix03-target"), body: "関係の相手" });
  const id = await publish(site, { title: marker("aix03"), body: `本文 ${marker("aix03-body")}`, fields: { summary, related: [await groupOf(site, target)] } });
  const markdown = await markdownOf(site, id);
  // 対照: devNote が付いたフィールドの値そのものは載っている
  expect(has(markdown, summary), "Markdown版に、指定したフィールド summary の値がない").toBe(true);
  expectNotLeaked(markdown, DEVNOTE_MARKER, "Markdown版");
  expectNotLeaked(await markdownOf(site, target), DEVNOTE_MARKER, "関係の相手のMarkdown版");
  expectNotLeaked(await catalogOf(site), DEVNOTE_MARKER, "目録");
});
