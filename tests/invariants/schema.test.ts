// スキーマとデプロイの約束: INV-SCH-01〜07
//
// デプロイはテンプレートを使うすべてのワークスペースに効くため、テストごとに別の名前のテンプレートを
// 作ってデプロイし、そのテンプレートのワークスペースで確かめる。デプロイの拒否は、CLIを通さず
// デプロイのAPIを直接呼んで確かめる（13-security.md）。

import { randomUUID } from "node:crypto";
import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asBearer, asSession, type IssuedSecret, type RegisteredQuery, type Session } from "./support/driver.js";
import { expectNotLeaked, expectRefused, expectSuccess } from "./support/assert.js";
import { simpleBody } from "./support/body.js";
import { cloneTemplate, DESCRIPTION_MARKER, DEVNOTE_MARKER, postType, type TemplateSpec } from "./support/template.js";
import { lazyWorld, marker } from "./support/world.js";

const world = lazyWorld();
const templateName = (label: string) => `inv-${label}-${randomUUID().slice(0, 8)}`;

async function deployer() {
  const w = await world();
  const token = await w.yomi.deployToken(w.serverAdminSession, w.serverAdmin);
  return {
    deploy: (t: TemplateSpec, options?: { allowDestructive?: boolean; allowBrokenQueries?: boolean }) => w.yomi.deployTemplateRaw(asBearer(token), t, options),
  };
}

interface TemplateWorkspace {
  id: string;
  adminSession: Session;
  editorSession: Session;
  readKey: IssuedSecret;
  registerKey: IssuedSecret;
  listQuery: RegisteredQuery;
  entryMarker: string;
}

/** テンプレートを使うワークスペースを作り、記事を1件公開しておく。 */
async function workspaceOn(template: string, entryFields: Record<string, unknown> = {}): Promise<TemplateWorkspace> {
  const { yomi, serverAdmin, serverAdminSession } = await world();
  await yomi.reauthenticate(serverAdminSession, serverAdmin);
  const { id } = await yomi.createWorkspace(serverAdminSession, `${template}-ws-${randomUUID().slice(0, 6)}`, { template });
  const admin = await yomi.createMember(serverAdminSession, id, "admin");
  const adminSession = await yomi.login(admin);
  const editor = await yomi.createMember(adminSession, id, "editor");
  const editorSession = await yomi.login(editor);
  const entryMarker = marker("sch");
  const e = await yomi.createEntry(editorSession, id, { title: entryMarker, body: simpleBody(entryMarker), fields: entryFields });
  await yomi.publishEntry(editorSession, id, e.id);
  await yomi.reauthenticate(adminSession, admin);
  const readKey = await yomi.createApiKey(adminSession, id, ["read"]);
  const registerKey = await yomi.createApiKey(adminSession, id, ["register"]);
  const listQuery = await yomi.registerQuery(id, registerKey.secret, { kind: "list" });
  return { id, adminSession, editorSession, readKey, registerKey, listQuery, entryMarker };
}

const removeField = (t: TemplateSpec, name: string) => {
  const post = postType(t);
  post.fields = post.fields.filter((f) => f.name !== name);
};

inv("inv_sch_01", "`devNote` の内容は、配信APIとスキーマ情報を返すAPIの応答に含まれない", async () => {
  const { yomi, a } = await world();
  // 対照: スキーマ情報には description が載っている（見る場所が正しい）
  const info = await yomi.schemaInfoRaw(asSession(a.editorSession), a.id);
  expectSuccess(info, "スキーマ情報の取得");
  expect(info.text.includes(DESCRIPTION_MARKER), "スキーマ情報に description が載っていない").toBe(true);

  for (const [who, session] of [["編集者", a.editorSession], ["管理者", a.adminSession]] as const) {
    expectNotLeaked(await yomi.schemaInfoRaw(asSession(session), a.id), DEVNOTE_MARKER, `${who}のスキーマ情報`);
  }
  const summaryQuery = await yomi.registerQuery(a.id, a.registerKey.secret, { kind: "list", fields: ["summary", "subtitle"] });
  for (const [what, res] of [
    ["一覧", await yomi.deliveryRaw(a.id, a.readKey.secret, a.listQuery, { locale: "ja", limit: 5 })],
    ["1件", await yomi.deliveryRaw(a.id, a.readKey.secret, a.byIdQuery, { id: a.entryId, locale: "ja" })],
    ["summaryを含む一覧", await yomi.deliveryRaw(a.id, a.readKey.secret, summaryQuery, { locale: "ja", limit: 5 })],
  ] as const) {
    expectNotLeaked(res, DEVNOTE_MARKER, `配信API（${what}）`);
  }
});

inv("inv_sch_02", "破壊的なスキーマ変更（フィールドの削除、型の変更など）は、明示的な指定がない限り、CLIを通さずにデプロイのAPIを直接呼んだ場合も拒否される", async () => {
  const { yomi } = await world();
  const { deploy } = await deployer();
  const name = templateName("sch02");
  expectSuccess(await deploy(cloneTemplate(name)), "最初のデプロイ");
  const ws = await workspaceOn(name, { subtitle: "副題" });
  const schemaHas = async (field: string) => (await yomi.schemaInfoRaw(asSession(ws.adminSession), ws.id)).text.includes(field);

  const breaking: [string, TemplateSpec][] = [
    ["フィールドの削除", cloneTemplate(name, (t) => removeField(t, "subtitle"))],
    ["型の変更", cloneTemplate(name, (t) => (postType(t).fields.find((f) => f.name === "subtitle")!.type = "richtext"))],
    ["コンテンツタイプの削除", cloneTemplate(name, (t) => (t.contentTypes = []))],
  ];
  for (const [label, t] of breaking) {
    expectRefused(await deploy(t), `明示的な指定のない${label}`);
    expect(await schemaHas("subtitle"), `${label}のデプロイが拒否された後に、スキーマが変わった`).toBe(true);
    expectSuccess(await yomi.deliveryRaw(ws.id, ws.readKey.secret, ws.listQuery, { locale: "ja", limit: 5 }), `${label}の拒否の後の配信`);
  }

  // 対照: 追加は通る。明示的に指定すれば破壊的な変更も通る
  expectSuccess(await deploy(cloneTemplate(name, (t) => postType(t).fields.push({ name: "added_field", type: "text" }))), "フィールドの追加");
  expectSuccess(
    await deploy(cloneTemplate(name, (t) => { postType(t).fields.push({ name: "added_field", type: "text" }); removeField(t, "subtitle"); }), { allowDestructive: true }),
    "明示的に指定したフィールドの削除",
  );
});

// --- INV-SCH-03, 04, 05, 07: デプロイは、テンプレートを使うすべてのワークスペースに、全部適用するか、全部やめるか ---
//
// 2つのワークスペースが同じテンプレートを使う。どちらの側でも、スキーマ情報・クエリの登録・編集で、
// 新しいスキーマが適用されたかどうかを確かめる。

/** 新しいフィールドが、そのワークスペースのスキーマにあるか（スキーマ情報・クエリの登録・記事の作成の3つで確かめる）。 */
async function expectSchemaHas(ws: TemplateWorkspace, field: string, has: boolean, label: string) {
  const { yomi } = await world();
  expect((await yomi.schemaInfoRaw(asSession(ws.adminSession), ws.id)).text.includes(field), `${label}: スキーマ情報に ${field} が${has ? "ない" : "ある"}`).toBe(has);
  const registered = await yomi.registerQueryRaw(ws.id, ws.registerKey.secret, { kind: "list", fields: [field] });
  if (has) expectSuccess(registered, `${label}: ${field} を使うクエリの登録`);
  else expectRefused(registered, `${label}: ${field} を使うクエリの登録`);
}

/** 配信と編集が続けられること。 */
async function expectServingAndEditing(ws: TemplateWorkspace, label: string) {
  const { yomi } = await world();
  const res = await yomi.deliveryRaw(ws.id, ws.readKey.secret, ws.listQuery, { locale: "ja", limit: 5 });
  expectSuccess(res, `${label}: 配信`);
  expect(res.text.includes(ws.entryMarker), `${label}: 配信に公開中の記事がない`).toBe(true);
  expectSuccess(
    await yomi.createEntryRaw(asSession(ws.editorSession), ws.id, { title: marker("sch05"), body: simpleBody("x"), fields: { subtitle: "s" } }),
    `${label}: 記事の作成`,
  );
  const entries = await yomi.listEntries(ws.editorSession, ws.id);
  const draft = await yomi.getDraft(ws.editorSession, ws.id, entries[0]!.id);
  expectSuccess(await yomi.saveDraftRaw(asSession(ws.editorSession), ws.id, entries[0]!.id, { fields: { subtitle: "追記" } }, draft.revision), `${label}: 下書きの保存`);
}

inv("inv_sch_03", "登録済みのPersisted Queryを壊すスキーマ変更のデプロイは、明示的な指定がない限り全体が拒否され、そのテンプレートを使うどのワークスペースにも適用されない。明示的な指定があれば、すべてのワークスペースに適用される。CLIを通さずにデプロイのAPIを直接呼んだ場合も同じ", async () => {
  const { yomi } = await world();
  const { deploy } = await deployer();
  const name = templateName("sch03");
  expectSuccess(await deploy(cloneTemplate(name)), "最初のデプロイ");
  const withQuery = await workspaceOn(name, { summary: "要約" });
  const other = await workspaceOn(name, { summary: "要約" });
  const usesSummary = await yomi.registerQuery(withQuery.id, withQuery.registerKey.secret, { kind: "list", fields: ["summary"] });

  // 破壊的な変更そのものは明示的に許しても、1つのワークスペースのクエリが壊れるなら、全体が拒否される
  const withoutSummary = cloneTemplate(name, (t) => {
    removeField(t, "summary");
    postType(t).fields.push({ name: "added_sch03", type: "text" });
  });
  expectRefused(await deploy(withoutSummary, { allowDestructive: true }), "1つのワークスペースのクエリが壊れる変更のデプロイ");
  for (const [ws, label] of [[withQuery, "クエリを持つ側"], [other, "クエリを持たない側"]] as const) {
    await expectSchemaHas(ws, "added_sch03", false, label);
    await expectSchemaHas(ws, "summary", true, label);
  }
  expectSuccess(await yomi.deliveryRaw(withQuery.id, withQuery.readKey.secret, usesSummary, { locale: "ja", limit: 5 }), "拒否の後の、登録済みのクエリ");

  // 明示的な指定があれば、すべてのワークスペースに適用される
  expectSuccess(await deploy(withoutSummary, { allowDestructive: true, allowBrokenQueries: true }), "明示的な指定のあるデプロイ");
  for (const [ws, label] of [[withQuery, "クエリを持つ側"], [other, "クエリを持たない側"]] as const) {
    await expectSchemaHas(ws, "added_sch03", true, label);
    await expectSchemaHas(ws, "summary", false, label);
  }
});

// 一方のワークスペースだけ移行に失敗する変更: subtitle を必須にし、フィールド extra_field_v2 を足す。
// 失敗する側（P）には subtitle が空の記事があり、もう一方（Q）の記事はすべて subtitle を持つ。

interface FailedMigrationScene {
  failing: TemplateWorkspace;
  passing: TemplateWorkspace;
}

let scene: Promise<FailedMigrationScene> | null = null;
const NEW_FIELD = "extra_field_v2";

function failedMigrationScene(): Promise<FailedMigrationScene> {
  scene ??= (async () => {
    const { deploy } = await deployer();
    const name = templateName("sch-mig");
    expectSuccess(await deploy(cloneTemplate(name)), "最初のデプロイ");
    const failing = await workspaceOn(name, {}); // subtitle なし
    const passing = await workspaceOn(name, { subtitle: "副題あり" });
    const next = cloneTemplate(name, (t) => {
      const post = postType(t);
      post.fields.find((f) => f.name === "subtitle")!.required = true;
      post.fields.push({ name: NEW_FIELD, type: "text" });
    });
    expectRefused(await deploy(next, { allowDestructive: true }), "1つのワークスペースで移行に失敗するデプロイ");
    return { failing, passing };
  })();
  return scene;
}

inv("inv_sch_04", "1つのワークスペースでも移行に失敗したデプロイは、そのテンプレートを使うどのワークスペースにも適用されない（配信APIのスキーマと、編集で使えるフィールドは、デプロイの前のまま）", async () => {
  const { yomi } = await world();
  const { failing, passing } = await failedMigrationScene();
  for (const [ws, label] of [[failing, "移行に失敗した側"], [passing, "移行できたはずの側"]] as const) {
    await expectSchemaHas(ws, NEW_FIELD, false, label);
    expectRefused(
      await yomi.deliveryAdHocRaw(ws.id, ws.readKey.secret, { kind: "list", fields: [NEW_FIELD] }, { locale: "ja", limit: 5 }),
      `${label}: 新しいフィールドを使うクエリ`,
    );
    // subtitle はまだ必須ではない
    expectSuccess(await yomi.createEntryRaw(asSession(ws.editorSession), ws.id, { title: marker("sch04"), body: simpleBody("x"), fields: {} }), `${label}: subtitle のない記事の作成`);
  }
});

inv("inv_sch_05", "拒否されたデプロイや、移行に失敗したデプロイの後も、そのテンプレートを使うすべてのワークスペースで、配信APIは応答を続け、記事の作成と下書きの保存もできる", async () => {
  const { failing, passing } = await failedMigrationScene();
  await expectServingAndEditing(failing, "移行に失敗した側");
  await expectServingAndEditing(passing, "移行できたはずの側");

  // 登録済みのクエリが壊れて拒否されたデプロイの後も同じ
  const { yomi } = await world();
  const { deploy } = await deployer();
  const name = templateName("sch05");
  expectSuccess(await deploy(cloneTemplate(name)), "最初のデプロイ");
  const withQuery = await workspaceOn(name, { summary: "要約" });
  const other = await workspaceOn(name, { summary: "要約" });
  await yomi.registerQuery(withQuery.id, withQuery.registerKey.secret, { kind: "list", fields: ["summary"] });
  expectRefused(await deploy(cloneTemplate(name, (t) => removeField(t, "summary")), { allowDestructive: true }), "クエリが壊れるデプロイ");
  await expectServingAndEditing(withQuery, "クエリを持つ側");
  await expectServingAndEditing(other, "クエリを持たない側");
});

inv("inv_sch_07", "適用されたデプロイは、そのテンプレートを使うすべてのワークスペースに適用される（どのワークスペースでも、配信APIのスキーマと、編集で使えるフィールドが新しいものになる）", async () => {
  const { yomi } = await world();
  const { deploy } = await deployer();
  const name = templateName("sch07");
  expectSuccess(await deploy(cloneTemplate(name)), "最初のデプロイ");
  const first = await workspaceOn(name, { subtitle: "副題" });
  const second = await workspaceOn(name, { subtitle: "副題" });
  expectSuccess(await deploy(cloneTemplate(name, (t) => postType(t).fields.push({ name: "added_sch07", type: "text" }))), "フィールドを足すデプロイ");
  for (const [ws, label] of [[first, "1つ目"], [second, "2つ目"]] as const) {
    await expectSchemaHas(ws, "added_sch07", true, label);
    expectSuccess(
      await yomi.createEntryRaw(asSession(ws.editorSession), ws.id, { title: marker("sch07"), body: simpleBody("x"), fields: { subtitle: "s", added_sch07: "新" } }),
      `${label}: 新しいフィールドを使う記事の作成`,
    );
  }
});

inv("inv_sch_06", "許可された文字以外を含むコンテンツタイプ名やフィールド名のスキーマは、CLIを通さずにデプロイのAPIを直接呼んだ場合も拒否される", async () => {
  const { deploy } = await deployer();
  // 英小文字・数字・アンダースコア以外を含む名前と、空・長すぎる名前
  const badNames = ["Post", "post-name", "post name", 'post";drop table x;--', "ポスト", "post$", "post.x", "", "a".repeat(100)];

  for (const bad of badNames) {
    const name = templateName("sch06");
    expectRefused(await deploy(cloneTemplate(name, (t) => (postType(t).name = bad))), `コンテンツタイプ名 ${JSON.stringify(bad)}`);
    expectRefused(
      await deploy(cloneTemplate(templateName("sch06"), (t) => postType(t).fields.push({ name: bad, type: "text" }))),
      `フィールド名 ${JSON.stringify(bad)}`,
    );
  }
  // 対照: 英小文字・数字・アンダースコアの名前は通る
  expectSuccess(
    await deploy(cloneTemplate(templateName("sch06"), (t) => postType(t).fields.push({ name: "valid_name_2", type: "text" }))),
    "正しいフィールド名",
  );
});
