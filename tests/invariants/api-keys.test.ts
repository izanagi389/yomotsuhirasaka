// APIキーの約束: INV-API-01, 02, INV-KEY-01, 04

import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asSession } from "./support/driver.js";
import { expectNotLeaked, expectRejected, expectSuccess } from "./support/assert.js";
import { operationsOf } from "./support/operations.js";
import { lazyWorld, sleep } from "./support/world.js";

const world = lazyWorld();

inv("inv_api_01", "APIキーのない配信APIへのリクエストは拒否される", async () => {
  const { yomi, a } = await world();

  // 対照: キーがあれば取得できる
  expectSuccess(await yomi.deliveryRaw(a.id, a.readKey.secret, a.listQuery), "キーのある配信");

  for (const query of [a.listQuery, a.byIdQuery]) {
    const res = await yomi.deliveryRaw(a.id, null, query, { id: a.entryId });
    expectRejected(res, "キーのない配信");
    expectNotLeaked(res, a.secretMarker, "キーのない配信");
  }
  const empty = await yomi.deliveryRaw(a.id, "", a.listQuery);
  expectRejected(empty, "空のキーでの配信");
  expectNotLeaked(empty, a.secretMarker, "空のキーでの配信");
});

inv("inv_api_02", "APIキーのスコープに含まれない操作は、そのキーでは実行できない", async () => {
  const { yomi, a } = await world();

  // 読み取り用のキーでは、クエリを登録できない（対照: 登録用のキーなら登録できる）
  expectSuccess(await yomi.registerQueryRaw(a.id, a.registerKey.secret, { kind: "list" }), "登録用のキーでのクエリの登録");
  expectRejected(await yomi.registerQueryRaw(a.id, a.readKey.secret, { kind: "list" }), "読み取り用のキーでのクエリの登録");

  // 配信APIのキーでは、管理APIを使えない
  const target = { workspaceId: a.id, entryId: a.entryId, userId: a.editor.userId, apiKeyId: a.readKey.id };
  for (const key of [a.readKey, a.registerKey]) {
    expectRejected(await yomi.meRaw({ kind: "bearer", token: key.secret }), "配信APIのキーでの本人情報の取得");
    for (const op of operationsOf(yomi).filter((o) => !o.mutating)) {
      const res = await op.call({ kind: "bearer", token: key.secret }, target);
      expectRejected(res, `配信APIのキーでの ${op.name}`);
      expectNotLeaked(res, a.secretMarker, `配信APIのキーでの ${op.name}`);
    }
  }
});

inv("inv_key_01", "発行した配信APIキーと個人トークンは、発行時の応答以外では管理APIから取得できない", async () => {
  const w = await world();
  const { yomi, a } = w;

  await yomi.reauthenticate(a.adminSession, a.admin);
  const key = await yomi.createApiKey(a.adminSession, a.id, ["read"]);
  const token = await yomi.createPersonalToken(a.adminSession, [a.id]);

  // 対照: 発行時の応答には含まれ、実際に使える
  expect(key.response.text.includes(key.secret), "発行時の応答にAPIキーが含まれていない").toBe(true);
  expect(token.response.text.includes(token.secret), "発行時の応答に個人トークンが含まれていない").toBe(true);
  expectSuccess(await yomi.deliveryRaw(a.id, key.secret, a.listQuery), "発行したキーでの配信");
  expectSuccess(await yomi.meRaw({ kind: "bearer", token: token.secret }), "発行した個人トークンでの本人情報の取得");

  // 発行後は、読み取りの操作のどれからも取得できない（対象にこのキーを指定する）
  const target = { workspaceId: a.id, entryId: a.entryId, userId: a.admin.userId, apiKeyId: key.id };
  for (const [who, session] of [["ワークスペースの管理者", a.adminSession], ["サーバー管理者", w.serverAdminSession]] as const) {
    for (const op of operationsOf(yomi).filter((o) => !o.mutating)) {
      const res = await op.call(asSession(session), target);
      expectNotLeaked(res, key.secret, `${who}の ${op.name}（APIキー）`);
      expectNotLeaked(res, token.secret, `${who}の ${op.name}（個人トークン）`);
    }
  }
});

inv("inv_key_04", "有効期限の切れたAPIキーは使えない", async () => {
  const { yomi, a } = await world();
  await yomi.reauthenticate(a.adminSession, a.admin);
  const key = await yomi.createApiKey(a.adminSession, a.id, ["read"], { expiresAt: new Date(Date.now() + 3_000) });
  expectSuccess(await yomi.deliveryRaw(a.id, key.secret, a.listQuery), "有効期限内のAPIキー");
  await sleep(5);
  const res = await yomi.deliveryRaw(a.id, key.secret, a.listQuery);
  expectRejected(res, "有効期限の切れたAPIキー");
  expectNotLeaked(res, a.secretMarker, "有効期限の切れたAPIキー");
});
