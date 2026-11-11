// ワークスペースの削除の約束: INV-TEN-07, 08, INV-DB-05（2026-11-12、持ち主の判断。M5 の例外）
//
// 削除はすぐに完全に行う（記事、メディア、APIキー、DBのスキーマとロール）。監査ログの行は通常の保存期間まで残る。
// 舞台の A・B は消さず、テストごとに使い捨てのワークスペースを作って消す。

import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asSession, driver, type StartedServer } from "./support/driver.js";
import { expectRefused, expectRejected, expectSuccess } from "./support/assert.js";
import { pngImage } from "./support/images.js";
import { disposableWorkspace, lazyWorld, sleep } from "./support/world.js";

const world = lazyWorld();
const REAUTH_WINDOW = 2;

inv(
  "inv_ten_07",
  "サーバー管理者以外は、ワークスペースを削除できない。サーバー管理者でも、ワークスペースの名前の入力と直前の本人の確認がなければ削除されない",
  async () => {
    const w = await world();
    const { yomi, serverAdmin, serverAdminSession } = w;
    const ws = await disposableWorkspace(w, "TEN07");
    const stillThere = async (what: string) =>
      expectSuccess(await yomi.deliveryRaw(ws.id, ws.readKey.secret, ws.listQuery), `${what}の後の配信（削除されていないこと）`);

    // サーバー管理者以外はできない（名前を正しく入れ、本人の確認を済ませても）
    await yomi.reauthenticate(ws.adminSession, ws.admin);
    expectRejected(await yomi.deleteWorkspaceRaw(asSession(ws.adminSession), ws.id, { confirmName: ws.name }), "ワークスペースの管理者による削除");
    expectRejected(await yomi.deleteWorkspaceRaw(asSession(ws.editorSession), ws.id, { confirmName: ws.name }), "編集者による削除");
    const token = await yomi.createPersonalToken(ws.adminSession, [ws.id]);
    expectRejected(await yomi.deleteWorkspaceRaw({ kind: "bearer", token: token.secret }, ws.id, { confirmName: ws.name }), "管理者の個人トークンによる削除");
    await stillThere("サーバー管理者以外による削除");

    // サーバー管理者でも、名前の入力がなければ・違えば削除されない
    await yomi.reauthenticate(serverAdminSession, serverAdmin);
    expectRefused(await yomi.deleteWorkspaceRaw(asSession(serverAdminSession), ws.id, {}), "名前を入れない削除");
    expectRefused(await yomi.deleteWorkspaceRaw(asSession(serverAdminSession), ws.id, { confirmName: `${ws.name}-x` }), "違う名前を入れた削除");
    await stillThere("名前のない・違う削除");

    // 直前の本人の確認がなければ削除されない（再認証の有効時間を2秒にしたサーバーで）
    const server: StartedServer = await driver.startServer({ ...(await driver.validServerConfig()), reauthWindowSeconds: REAUTH_WINDOW });
    try {
      expect(server.outcome, `再認証の有効時間を短くしたサーバーが起動しない:\n${server.output}`).toBe("ready");
      const stale = await server.client!.login(serverAdmin);
      await sleep(REAUTH_WINDOW + 2);
      expectRefused(await server.client!.deleteWorkspaceRaw(asSession(stale), ws.id, { confirmName: ws.name }), "本人の確認の有効時間を過ぎた削除");
    } finally {
      await server.stop();
    }
    await stillThere("本人の確認のない削除");

    // 対照: サーバー管理者が、名前を入れて本人の確認をすれば削除できる
    await yomi.reauthenticate(serverAdminSession, serverAdmin);
    expectSuccess(await yomi.deleteWorkspaceRaw(asSession(serverAdminSession), ws.id, { confirmName: ws.name }), "サーバー管理者による削除");
  },
  120_000,
);

inv(
  "inv_ten_08",
  "削除したワークスペースの記事・メディア・APIキーは、配信API・管理API・メディアの配信のどこからも返らない。ほかのワークスペースには影響しない",
  async () => {
    const w = await world();
    const { yomi, serverAdmin, serverAdminSession, a } = w;
    const ws = await disposableWorkspace(w, "TEN08");
    const media = await yomi.uploadMedia(ws.editorSession, ws.id, { filename: "ten08.png", bytes: pngImage(8, 8), contentType: "image/png" });

    // 対照: 削除の前は返る
    expectSuccess(await yomi.deliveryRaw(ws.id, ws.readKey.secret, ws.byIdQuery, { id: ws.entryId, locale: "ja" }), "削除の前の配信");
    expectSuccess(await yomi.getEntryRaw(asSession(ws.editorSession), ws.id, ws.entryId), "削除の前の管理APIでの取得");
    expectSuccess(await yomi.fetchMediaRaw(media.url), "削除の前のメディアの取得");

    await yomi.reauthenticate(serverAdminSession, serverAdmin);
    expectSuccess(await yomi.deleteWorkspaceRaw(asSession(serverAdminSession), ws.id, { confirmName: ws.name }), "サーバー管理者による削除");

    // 配信API（APIキー）、管理API（所属していた人とサーバー管理者）、メディアの配信のどこからも返らない
    for (const [what, res] of [
      ["削除の後の一覧の配信", await yomi.deliveryRaw(ws.id, ws.readKey.secret, ws.listQuery)],
      ["削除の後の記事の配信", await yomi.deliveryRaw(ws.id, ws.readKey.secret, ws.byIdQuery, { id: ws.entryId, locale: "ja" })],
      ["削除の後の、所属していた編集者による記事の取得", await yomi.getEntryRaw(asSession(ws.editorSession), ws.id, ws.entryId)],
      ["削除の後の、サーバー管理者による記事の取得", await yomi.getEntryRaw(asSession(serverAdminSession), ws.id, ws.entryId)],
      ["削除の後のメディアの取得", await yomi.fetchMediaRaw(media.url)],
    ] as const) {
      expectRefused(res, what);
      expect(res.text.includes(ws.secretMarker), `${what} の応答に、削除した記事の内容が含まれている`).toBe(false);
    }

    // 対照: ほかのワークスペースは変わらず返る
    expectSuccess(await yomi.deliveryRaw(a.id, a.readKey.secret, a.byIdQuery, { id: a.entryId, locale: "ja" }), "ほかのワークスペースの配信");
    expectSuccess(await yomi.getEntryRaw(asSession(a.editorSession), a.id, a.entryId), "ほかのワークスペースの管理APIでの取得");
  },
  120_000,
);

inv("inv_db_05", "削除したワークスペースのDBのスキーマとロールは残らない", async () => {
  const w = await world();
  const { yomi, serverAdmin, serverAdminSession } = w;
  const ws = await disposableWorkspace(w, "DB05");
  const schema = await driver.db.workspaceSchema(ws.id);
  const role = await driver.db.workspaceRole(ws.id);
  // 対照: 削除の前はある
  expect(await driver.db.schemaExists(schema), "削除の前にスキーマがない").toBe(true);
  expect(await driver.db.roleExists(role), "削除の前にロールがない").toBe(true);

  await yomi.reauthenticate(serverAdminSession, serverAdmin);
  expectSuccess(await yomi.deleteWorkspaceRaw(asSession(serverAdminSession), ws.id, { confirmName: ws.name }), "サーバー管理者による削除");

  expect(await driver.db.schemaExists(schema), "削除したワークスペースのスキーマが残っている").toBe(false);
  expect(await driver.db.roleExists(role), "削除したワークスペースのロールが残っている").toBe(false);
});
