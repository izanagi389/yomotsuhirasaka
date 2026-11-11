// テンプレートの切り替えの約束: INV-SCH-08, 09（2026-11-11、持ち主の判断。M5 の例外）
//
// 切り替えは、ワークスペースを、今のテンプレートの版から別のテンプレートの最新の版へ移す。テストごとに別の名前の
// テンプレートを2つデプロイし、片方を使うワークスペースで確かめる。切り替えは CLI を通さず、APIを直接呼ぶ。

import { randomUUID } from "node:crypto";
import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asBearer, asSession, driver, type Session, type StartedServer, type YomiClient } from "./support/driver.js";
import { expectRefused, expectSuccess } from "./support/assert.js";
import { simpleBody } from "./support/body.js";
import { cloneTemplate, postType, type TemplateSpec } from "./support/template.js";
import { lazyWorld, marker, sleep } from "./support/world.js";

const world = lazyWorld();
const templateName = (label: string) => `inv-${label}-${randomUUID().slice(0, 8)}`;
const REAUTH_WINDOW = 2;

async function deployBoth(withSummary: string, withoutSummary: string): Promise<void> {
  const w = await world();
  const token = await w.yomi.deployToken(w.serverAdminSession, w.serverAdmin);
  const removeSummary = (t: TemplateSpec) => {
    const post = postType(t);
    post.fields = post.fields.filter((f) => f.name !== "summary");
  };
  expectSuccess(await w.yomi.deployTemplateRaw(asBearer(token), cloneTemplate(withSummary)), "要約のあるテンプレートのデプロイ");
  expectSuccess(await w.yomi.deployTemplateRaw(asBearer(token), cloneTemplate(withoutSummary, removeSummary)), "要約のないテンプレートのデプロイ");
}

/** テンプレートを使うワークスペースを作り、要約のある記事を1件公開しておく。 */
async function workspaceOn(template: string, summary?: string): Promise<{ id: string; entryId: string; adminSession: Session; admin: Parameters<YomiClient["login"]>[0] }> {
  const { yomi, serverAdmin, serverAdminSession } = await world();
  await yomi.reauthenticate(serverAdminSession, serverAdmin);
  const { id } = await yomi.createWorkspace(serverAdminSession, `${template}-ws`, { template });
  const admin = await yomi.createMember(serverAdminSession, id, "admin");
  const adminSession = await yomi.login(admin);
  const title = marker("switch");
  const e = await yomi.createEntry(adminSession, id, { title, body: simpleBody(title), fields: summary === undefined ? {} : { summary } });
  await yomi.publishEntry(adminSession, id, e.id);
  return { id, entryId: e.id, adminSession, admin };
}

async function fieldsOf(session: Session, workspaceId: string, entryId: string): Promise<Record<string, unknown>> {
  const { yomi } = await world();
  const res = await yomi.getEntryRaw(asSession(session), workspaceId, entryId);
  expectSuccess(res, "記事の取得");
  return (JSON.parse(res.text) as { fields?: Record<string, unknown> }).fields ?? {};
}

inv(
  "inv_sch_08",
  "データが消えるテンプレートの切り替え（新しいテンプレートに対応づけのないフィールドやコンテンツタイプがある切り替え）は、明示的な指定と、直前の本人の確認がなければ行われない。CLIを通さずにAPIを直接呼んだ場合も同じ",
  async () => {
    const w = await world();
    const from = templateName("sw08-from");
    const to = templateName("sw08-to");
    await deployBoth(from, to);
    const summary = marker("sw08-summary");
    const ws = await workspaceOn(from, summary);
    const versionsBefore = await w.yomi.listVersions(ws.adminSession, ws.id, ws.entryId);
    await w.yomi.reauthenticate(w.serverAdminSession, w.serverAdmin);

    // 明示的な指定がなければ、データが消える切り替えは行われない
    expectRefused(await w.yomi.switchTemplateRaw(asSession(w.serverAdminSession), ws.id, { template: to }), "明示的な指定のない、データが消える切り替え");
    expect(await w.yomi.workspaceTemplate(w.serverAdminSession, ws.id), "明示的な指定なしに、テンプレートが切り替わった").toBe(from);
    expect((await fieldsOf(ws.adminSession, ws.id, ws.entryId))["summary"], "明示的な指定なしに、要約が消えた").toBe(summary);

    // 試行は何も変えない
    expectSuccess(await w.yomi.switchTemplateRaw(asSession(w.serverAdminSession), ws.id, { template: to, dryRun: true, allowDestructive: true }), "切り替えの試行");
    expect(await w.yomi.workspaceTemplate(w.serverAdminSession, ws.id), "試行で、テンプレートが切り替わった").toBe(from);

    // 直前の本人の確認がなければ、明示的な指定があっても行われない（再認証の有効時間を2秒にしたサーバーで）
    const server: StartedServer = await driver.startServer({ ...(await driver.validServerConfig()), reauthWindowSeconds: REAUTH_WINDOW });
    try {
      expect(server.outcome, `再認証の有効時間を短くしたサーバーが起動しない:\n${server.output}`).toBe("ready");
      const stale = await server.client!.login(w.serverAdmin);
      await sleep(REAUTH_WINDOW + 2);
      expectRefused(
        await server.client!.switchTemplateRaw(asSession(stale), ws.id, { template: to, allowDestructive: true }),
        "本人の確認の有効時間を過ぎた、データが消える切り替え",
      );
    } finally {
      await server.stop();
    }
    expect(await w.yomi.workspaceTemplate(w.serverAdminSession, ws.id), "本人の確認なしに、テンプレートが切り替わった").toBe(from);

    // 対照: 明示的な指定と本人の確認があれば行われ、消えるのは対応づけのないフィールドだけ
    await w.yomi.reauthenticate(w.serverAdminSession, w.serverAdmin);
    expectSuccess(
      await w.yomi.switchTemplateRaw(asSession(w.serverAdminSession), ws.id, { template: to, allowDestructive: true }),
      "明示的な指定と本人の確認のある切り替え",
    );
    expect(await w.yomi.workspaceTemplate(w.serverAdminSession, ws.id)).toBe(to);
    const after = await fieldsOf(ws.adminSession, ws.id, ws.entryId);
    expect(after["summary"], "対応づけのないフィールドが残った").toBeUndefined();
    expect(String(after["title"] ?? ""), "対応づけたフィールド（タイトル）が引き継がれていない").toMatch(/switch/);
    // 版の履歴（番号と数）は残る
    const versionsAfter = await w.yomi.listVersions(ws.adminSession, ws.id, ws.entryId);
    expect(versionsAfter.map((v) => v.id), "切り替えで版の履歴が変わった").toEqual(versionsBefore.map((v) => v.id));

    // 対照: データが消えない切り替え（フィールドが増えるだけ）は、明示的な指定なしで行われる
    expectSuccess(await w.yomi.switchTemplateRaw(asSession(w.serverAdminSession), ws.id, { template: from }), "データが消えない切り替え");
    expect(await w.yomi.workspaceTemplate(w.serverAdminSession, ws.id)).toBe(from);
  },
  240_000,
);

inv(
  "inv_sch_09",
  "テンプレートの切り替えは、サーバー管理者と、重い権限「テンプレートの切り替え」を付与されたワークスペースの管理者だけが行える。この権限を付与できるのはサーバー管理者だけ",
  async () => {
    const w = await world();
    const from = templateName("sw09-from");
    const to = templateName("sw09-to");
    await deployBoth(to, from); // from は要約なし、to は要約あり（データが消えない切り替え）
    const ws = await workspaceOn(from);
    const editor = await w.yomi.createMember(ws.adminSession, ws.id, "editor");
    const editorSession = await w.yomi.login(editor);
    await w.yomi.reauthenticate(ws.adminSession, ws.admin);

    // 付与のない管理者と、編集者はできない
    expectRefused(await w.yomi.switchTemplateRaw(asSession(ws.adminSession), ws.id, { template: to }), "権限の付与のない管理者による切り替え");
    expectRefused(await w.yomi.switchTemplateRaw(asSession(editorSession), ws.id, { template: to }), "編集者による切り替え");
    expect(await w.yomi.workspaceTemplate(w.serverAdminSession, ws.id)).toBe(from);

    // ワークスペースの管理者は、この権限を自分に付与できない（付与はサーバー管理者だけ）
    const me = await w.yomi.me(ws.adminSession);
    expectRefused(await w.yomi.setMemberGrantsRaw(asSession(ws.adminSession), ws.id, me.userId, ["switchTemplate"]), "管理者による「テンプレートの切り替え」の自分への付与");

    // サーバー管理者に付与された管理者はできる
    await w.yomi.reauthenticate(w.serverAdminSession, w.serverAdmin);
    expectSuccess(await w.yomi.setMemberGrantsRaw(asSession(w.serverAdminSession), ws.id, me.userId, ["switchTemplate"]), "権限の付与");
    await w.yomi.reauthenticate(ws.adminSession, ws.admin);
    expectSuccess(await w.yomi.switchTemplateRaw(asSession(ws.adminSession), ws.id, { template: to }), "権限を付与された管理者による切り替え");
    expect(await w.yomi.workspaceTemplate(w.serverAdminSession, ws.id)).toBe(to);

    // 対照: サーバー管理者はできる
    expectSuccess(await w.yomi.switchTemplateRaw(asSession(w.serverAdminSession), ws.id, { template: from, allowDestructive: true }), "サーバー管理者による切り替え");
  },
  180_000,
);
