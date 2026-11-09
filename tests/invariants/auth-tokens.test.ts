// キーとトークンの約束: INV-AUTH-09, 11, 18, 20

import { expect } from "vitest";
import { randomUUID } from "node:crypto";
import { inv } from "./support/inv.js";
import { asBearer, asSession } from "./support/driver.js";
import { expectNotLeaked, expectRejected, expectSuccess } from "./support/assert.js";
import { simpleBody } from "./support/body.js";
import { cloneTemplate } from "./support/template.js";
import { lazyWorld, marker, sleep } from "./support/world.js";

const world = lazyWorld();

const newEntry = () => ({ title: marker("token-entry"), body: simpleBody("トークンで作る記事") });

inv("inv_auth_09", "CLI用の個人トークンは、有効期限の後や失効の後は使えず、付与された範囲外の操作もできない", async () => {
  const w = await world();
  const { yomi, a, b } = w;

  // 有効期限
  const expiring = await yomi.createPersonalToken(a.adminSession, [a.id], { scopes: ["read"], expiresAt: new Date(Date.now() + 3_000) });
  expectSuccess(await yomi.meRaw(asBearer(expiring.secret)), "有効期限内の個人トークン");
  await sleep(5);
  expectRejected(await yomi.meRaw(asBearer(expiring.secret)), "有効期限の切れた個人トークン");
  expectRejected(await yomi.getEntryRaw(asBearer(expiring.secret), a.id, a.entryId), "有効期限の切れた個人トークンでの記事の取得");

  // 失効
  const revoked = await yomi.createPersonalToken(a.adminSession, [a.id], { scopes: ["read"] });
  expectSuccess(await yomi.meRaw(asBearer(revoked.secret)), "失効前の個人トークン");
  await yomi.revokePersonalToken(a.adminSession, revoked.id);
  expectRejected(await yomi.meRaw(asBearer(revoked.secret)), "失効した個人トークン");

  // 範囲: 読み取りだけのトークンでは書き込めない（対照: 書き込みの範囲なら書ける）
  const readOnly = await yomi.createPersonalToken(a.editorSession, [a.id], { scopes: ["read"] });
  expectRejected(await yomi.createEntryRaw(asBearer(readOnly.secret), a.id, newEntry()), "読み取りだけの個人トークンでの記事の作成");
  const writable = await yomi.createPersonalToken(a.editorSession, [a.id], { scopes: ["read", "write"] });
  expectSuccess(await yomi.createEntryRaw(asBearer(writable.secret), a.id, newEntry()), "書き込みの範囲の個人トークンでの記事の作成");
  // 範囲: 書き込みだけのトークンでは読めない（対照は、上の読み取りの範囲の有効期限の確認）
  const writeOnly = await yomi.createPersonalToken(a.editorSession, [a.id], { scopes: ["write"] });
  expectRejected(await yomi.getEntryRaw(asBearer(writeOnly.secret), a.id, a.entryId), "書き込みだけの個人トークンでの記事の取得");

  // 範囲: 選んだワークスペース以外では使えない（本人は両方に所属している）
  const both = await yomi.createMember(a.adminSession, a.id, "editor");
  await yomi.addExistingMember(b.adminSession, b.id, both, "editor");
  const bothSession = await yomi.login(both);
  const onlyA = await yomi.createPersonalToken(bothSession, [a.id], { scopes: ["read", "write"] });
  expectSuccess(await yomi.getEntryRaw(asBearer(onlyA.secret), a.id, a.entryId), "選んだワークスペースでの記事の取得");
  const other = await yomi.getEntryRaw(asBearer(onlyA.secret), b.id, b.entryId);
  expectRejected(other, "選んでいないワークスペースでの記事の取得");
  expectNotLeaked(other, b.secretMarker, "選んでいないワークスペースでの記事の取得");
  expectRejected(await yomi.createEntryRaw(asBearer(onlyA.secret), b.id, newEntry()), "選んでいないワークスペースでの記事の作成");
});

inv("inv_auth_11", "配信APIのキーや個人トークンをURLのクエリで送ったリクエストは拒否される", async () => {
  const { yomi, a } = await world();
  const key = a.readKey.secret;
  const token = await yomi.createPersonalToken(a.adminSession, [a.id], { scopes: ["read"] });
  const names = ["api_key", "apikey", "key", "token", "access_token"];

  // 対照: ヘッダーで送れば通る
  expectSuccess(await yomi.deliveryRaw(a.id, key, a.listQuery), "ヘッダーで送った配信APIのキー");
  expectSuccess(await yomi.meRaw(asBearer(token.secret)), "ヘッダーで送った個人トークン");

  for (const name of names) {
    // クエリだけで送る
    const onlyQuery = await yomi.deliveryRaw(a.id, null, a.listQuery, {}, { query: { [name]: key } });
    expectRejected(onlyQuery, `配信APIのキーをクエリ ${name} で送った配信`);
    expectNotLeaked(onlyQuery, a.secretMarker, `配信APIのキーをクエリ ${name} で送った配信`);
    // ヘッダーとクエリの両方で送る（URLに載った時点で漏れたものとして扱う）
    expectRejected(await yomi.deliveryRaw(a.id, key, a.listQuery, {}, { query: { [name]: key } }), `配信APIのキーをヘッダーとクエリ ${name} の両方で送った配信`);

    expectRejected(await yomi.meRaw({ kind: "none" }, { query: { [name]: token.secret } }), `個人トークンをクエリ ${name} で送った本人情報の取得`);
    expectRejected(await yomi.meRaw(asBearer(token.secret), { query: { [name]: token.secret } }), `個人トークンをヘッダーとクエリ ${name} の両方で送った本人情報の取得`);
  }
});

inv("inv_auth_18", "個人トークンは、本人がワークスペースから外された後はそのワークスペースで使えず、本人のロールを超える操作もできない", async () => {
  const { yomi, a, b } = await world();

  // ワークスペースから外された後
  const user = await yomi.createMember(a.adminSession, a.id, "editor");
  await yomi.addExistingMember(b.adminSession, b.id, user, "editor");
  const session = await yomi.login(user);
  const token = await yomi.createPersonalToken(session, [a.id, b.id], { scopes: ["read", "write"] });
  expectSuccess(await yomi.getEntryRaw(asBearer(token.secret), a.id, a.entryId), "外される前のAでの記事の取得");
  await yomi.reauthenticate(a.adminSession, a.admin);
  await yomi.removeMember(a.adminSession, a.id, user.userId);
  const afterRemoval = await yomi.getEntryRaw(asBearer(token.secret), a.id, a.entryId);
  expectRejected(afterRemoval, "外された後のAでの記事の取得");
  expectNotLeaked(afterRemoval, a.secretMarker, "外された後のAでの記事の取得");
  expectRejected(await yomi.createEntryRaw(asBearer(token.secret), a.id, newEntry()), "外された後のAでの記事の作成");
  // 対照: 外されていないBでは使える
  expectSuccess(await yomi.getEntryRaw(asBearer(token.secret), b.id, b.entryId), "外されていないBでの記事の取得");

  // ロールを下げられた後（編集者 → 閲覧者）
  await yomi.reauthenticate(b.adminSession, b.admin);
  await yomi.changeMemberRole(b.adminSession, b.id, user.userId, "viewer");
  expectRejected(await yomi.createEntryRaw(asBearer(token.secret), b.id, newEntry()), "閲覧者に下げられた後の記事の作成");
  expectSuccess(await yomi.getEntryRaw(asBearer(token.secret), b.id, b.entryId), "閲覧者に下げられた後の記事の取得");

  // 発行したときから、ロールを超える操作はできない（執筆者は公開できない）
  const author = await yomi.createMember(a.adminSession, a.id, "author");
  const authorSession = await yomi.login(author);
  const authorToken = await yomi.createPersonalToken(authorSession, [a.id], { scopes: ["read", "write"] });
  const created = await yomi.createEntry(authorSession, a.id, newEntry());
  expectRejected(await yomi.publishRaw(asBearer(authorToken.secret), a.id, created.id), "執筆者の個人トークンでの公開");
});

inv("inv_auth_20", "サーバー管理者以外の利用者とトークンでは、スキーマのデプロイを実行できない", async () => {
  const { yomi, serverAdmin, serverAdminSession, a } = await world();
  const template = () => cloneTemplate(`inv-auth-20-${randomUUID().slice(0, 8)}`);

  await yomi.reauthenticate(a.adminSession, a.admin);
  expectRejected(await yomi.deployTemplateRaw(asSession(a.adminSession), template()), "ワークスペースの管理者のセッションでのデプロイ");
  expectRejected(await yomi.deployTemplateRaw(asSession(a.editorSession), template()), "編集者のセッションでのデプロイ");

  const adminToken = await yomi.createPersonalToken(a.adminSession, [a.id], { scopes: ["read", "write"] });
  expectRejected(await yomi.deployTemplateRaw(asBearer(adminToken.secret), template()), "ワークスペースの管理者の個人トークンでのデプロイ");

  // デプロイの範囲は、サーバー管理者以外には付けられない（付けられたとしても、デプロイはできない）
  let deployScoped: string | null = null;
  try {
    deployScoped = (await yomi.createPersonalToken(a.adminSession, [a.id], { scopes: ["deploy"] })).secret;
  } catch {
    deployScoped = null;
  }
  if (deployScoped) {
    expectRejected(await yomi.deployTemplateRaw(asBearer(deployScoped), template()), "ワークスペースの管理者がデプロイの範囲を付けたトークンでのデプロイ");
  }

  // 対照: サーバー管理者のデプロイ用のトークンならデプロイできる
  const token = await yomi.deployToken(serverAdminSession, serverAdmin);
  const res = await yomi.deployTemplateRaw(asBearer(token), template());
  expectSuccess(res, "サーバー管理者のデプロイ用のトークンでのデプロイ");
  expect(res.status).toBeLessThan(300);
});
