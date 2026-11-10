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

inv("inv_sch_03", "登録済みのPersisted Queryを壊すスキーマ変更は、明示的な指定がない限り、そのクエリを持つワークスペースには適用されない（ほかのワークスペースには適用される）。CLIを通さずにデプロイのAPIを直接呼んだ場合も同じ", async () => {
  const { yomi } = await world();
  const { deploy } = await deployer();
  const name = templateName("sch03");
  expectSuccess(await deploy(cloneTemplate(name)), "最初のデプロイ");
  const withQuery = await workspaceOn(name, { summary: "要約" });
  const other = await workspaceOn(name, { summary: "要約" });
  const usesSummary = await yomi.registerQuery(withQuery.id, withQuery.registerKey.secret, { kind: "list", fields: ["summary"] });

  // 破壊的な変更そのものは明示的に許しても、登録済みのクエリを壊すワークスペースには適用されない
  const withoutSummary = cloneTemplate(name, (t) => removeField(t, "summary"));
  expectSuccess(await deploy(withoutSummary, { allowDestructive: true }), "1つのワークスペースのクエリが壊れる変更のデプロイ");
  expect(await yomi.migrationState(withQuery.adminSession, withQuery.id), "壊れるクエリを持つワークスペースに、変更が適用された").toBe("pending");
  expectSuccess(await yomi.deliveryRaw(withQuery.id, withQuery.readKey.secret, usesSummary, { locale: "ja", limit: 5 }), "適用されなかった側で、登録済みのクエリが動く");

  // ほかのワークスペースには適用される（1つのワークスペースのクエリで、全体のデプロイを止められない）
  expect(await yomi.migrationState(other.adminSession, other.id), "壊れるクエリを持たないワークスペースに、変更が適用されていない").toBe("ok");
  expectRefused(await yomi.registerQueryRaw(other.id, other.registerKey.secret, { kind: "list", fields: ["summary"] }), "適用された側で、削除したフィールドを使うクエリの登録");

  // 明示的な指定があれば、壊れるクエリを持つワークスペースにも適用される
  expectSuccess(await deploy(withoutSummary, { allowDestructive: true, allowBrokenQueries: true }), "明示的な指定のあるデプロイ");
  expect(await yomi.migrationState(withQuery.adminSession, withQuery.id), "明示的な指定があっても、変更が適用されていない").toBe("ok");
});

// --- INV-SCH-04, 05, 07: 一方のワークスペースだけ移行に失敗する状況を作る ---
//
// v1: subtitle は任意。v2: subtitle を必須にし、フィールド extra_field_v2 を足す。
// 失敗する側（P）には subtitle が空の記事があり、成功する側（Q）の記事はすべて subtitle を持つ。

interface MigrationScene {
  pending: TemplateWorkspace;
  migrated: TemplateWorkspace;
}

let scene: Promise<MigrationScene> | null = null;
const NEW_FIELD = "extra_field_v2";

function migrationScene(): Promise<MigrationScene> {
  scene ??= (async () => {
    const { yomi } = await world();
    const { deploy } = await deployer();
    const name = templateName("sch-mig");
    expectSuccess(await deploy(cloneTemplate(name)), "v1のデプロイ");
    const pending = await workspaceOn(name, {}); // subtitle なし
    const migrated = await workspaceOn(name, { subtitle: "副題あり" });
    const v2 = cloneTemplate(name, (t) => {
      const post = postType(t);
      post.fields.find((f) => f.name === "subtitle")!.required = true;
      post.fields.push({ name: NEW_FIELD, type: "text" });
    });
    await deploy(v2, { allowDestructive: true }); // 一部のワークスペースだけ失敗するため、応答の状態コードは問わない
    expect(await yomi.migrationState(pending.adminSession, pending.id), "subtitle が空の記事がある側が、移行待ちになっていない").toBe("pending");
    expect(await yomi.migrationState(migrated.adminSession, migrated.id), "subtitle がそろっている側が、移行を終えていない").toBe("ok");
    return { pending, migrated };
  })();
  return scene;
}

inv("inv_sch_04", "移行待ちのワークスペースでも配信APIは応答を続け、編集の操作だけが拒否される", async () => {
  const { yomi } = await world();
  const { pending } = await migrationScene();

  const res = await yomi.deliveryRaw(pending.id, pending.readKey.secret, pending.listQuery, { locale: "ja", limit: 5 });
  expectSuccess(res, "移行待ちのワークスペースの配信");
  expect(res.text.includes(pending.entryMarker), "移行待ちのワークスペースの配信に、公開中の記事がない").toBe(true);

  expectRefused(await yomi.createEntryRaw(asSession(pending.editorSession), pending.id, { title: "x", body: simpleBody("x"), fields: { subtitle: "s" } }), "移行待ちのワークスペースでの記事の作成");
  const entries = await yomi.listEntries(pending.editorSession, pending.id);
  const draft = await yomi.getDraft(pending.editorSession, pending.id, entries[0]!.id);
  expectRefused(await yomi.saveDraftRaw(asSession(pending.editorSession), pending.id, entries[0]!.id, { fields: { subtitle: "追記" } }, draft.revision), "移行待ちのワークスペースでの下書きの保存");
});

inv("inv_sch_05", "あるワークスペースの移行が失敗しても、他のワークスペースの移行結果と配信には影響しない", async () => {
  const { yomi } = await world();
  const { migrated } = await migrationScene();

  expectSuccess(await yomi.deliveryRaw(migrated.id, migrated.readKey.secret, migrated.listQuery, { locale: "ja", limit: 5 }), "移行に成功した側の配信");
  expect((await yomi.schemaInfoRaw(asSession(migrated.adminSession), migrated.id)).text.includes(NEW_FIELD), "移行に成功した側に、新しいフィールドがない").toBe(true);
  expectSuccess(await yomi.registerQueryRaw(migrated.id, migrated.registerKey.secret, { kind: "list", fields: [NEW_FIELD] }), "移行に成功した側で、新しいフィールドを使うクエリの登録");
  expectSuccess(
    await yomi.createEntryRaw(asSession(migrated.editorSession), migrated.id, { title: marker("sch05"), body: simpleBody("x"), fields: { subtitle: "必須", [NEW_FIELD]: "新" } }),
    "移行に成功した側での記事の作成",
  );
});

inv("inv_sch_07", "移行待ちのワークスペースの配信APIは、そのワークスペースに適用済みのバージョンのスキーマで応答する", async () => {
  const { yomi } = await world();
  const { pending } = await migrationScene();

  // 新しいフィールドは、移行待ちの側のスキーマにはまだない
  expectRefused(await yomi.registerQueryRaw(pending.id, pending.registerKey.secret, { kind: "list", fields: [NEW_FIELD] }), "移行待ちの側で、新しいフィールドを使うクエリの登録");
  const adHoc = await yomi.deliveryAdHocRaw(pending.id, pending.readKey.secret, { kind: "list", fields: [NEW_FIELD] }, { locale: "ja", limit: 5 });
  expectRefused(adHoc, "移行待ちの側での、新しいフィールドを使うクエリ");
  // 以前から登録してあるクエリ（適用済みのスキーマで有効なもの）は動く
  const res = await yomi.deliveryRaw(pending.id, pending.readKey.secret, pending.listQuery, { locale: "ja", limit: 5 });
  expectSuccess(res, "移行待ちの側の登録済みのクエリ");
  expect((await yomi.schemaInfoRaw(asSession(pending.adminSession), pending.id)).text.includes(NEW_FIELD), "移行待ちの側のスキーマ情報に、新しいフィールドがある").toBe(false);
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
