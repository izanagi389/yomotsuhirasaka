// 緊急停止と鍵の入れ替えの約束: INV-AUTH-23, INV-KEY-03
//
// どちらもインスタンス全体に効く操作なので、他のテストと共有しない新しいインスタンスで確かめる。

import { createHmac } from "node:crypto";
import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asBearer, asSession, driver, type IsolatedInstance, type ServerConfig, type StartedServer } from "./support/driver.js";
import { expectRejected, expectSuccess } from "./support/assert.js";
import { env } from "./support/env.js";
import { startReceiver } from "./support/net.js";

async function started(config: ServerConfig): Promise<StartedServer> {
  const server = await driver.startServer(config);
  expect(server.outcome, `サーバーが起動しない:\n${server.output}`).toBe("ready");
  return server;
}

inv(
  "inv_auth_23",
  "すべてのセッションと個人トークンを一斉に失効させる緊急停止の操作は、サーバー上のCLIからだけ実行でき、実行後はそれまでのセッションとトークンがすべて使えなくなる",
  async () => {
    const instance: IsolatedInstance = await driver.createIsolatedInstance();
    const server = await started(instance.config);
    try {
      const yomi = server.client!;
      const sa1 = await yomi.login(instance.serverAdmin);
      const sa2 = await yomi.login(instance.serverAdmin);
      await yomi.reauthenticate(sa1, instance.serverAdmin);
      const ws = await yomi.createWorkspace(sa1, "inv-auth-23");
      const member = await yomi.createMember(sa1, ws.id, "admin");
      const memberSession = await yomi.login(member);
      const token = await yomi.createPersonalToken(memberSession, [ws.id], { scopes: ["read"] });

      const sessions = [sa1, sa2, memberSession];
      for (const s of sessions) expectSuccess(await yomi.meRaw(asSession(s)), "緊急停止の前のセッション");
      expectSuccess(await yomi.meRaw(asBearer(token.secret)), "緊急停止の前の個人トークン");

      // ネットワーク越しの管理APIには、一斉失効の操作がない
      const names = yomi.adminOperations.map((o) => o.name.toLowerCase());
      expect(names.filter((n) => /revokeall|emergency|killall/.test(n)), "管理APIに一斉失効の操作がある").toEqual([]);

      const result = await driver.emergencyRevokeAllSessions(instance.config);
      expect(result.exitCode, `緊急停止のCLIが失敗した:\n${result.output}`).toBe(0);

      for (const s of sessions) expectRejected(await yomi.meRaw(asSession(s)), "緊急停止の後のセッション");
      expectRejected(await yomi.meRaw(asBearer(token.secret)), "緊急停止の後の個人トークン");

      // 対照: 新しくログインすれば使える
      expectSuccess(await yomi.meRaw(asSession(await yomi.login(member))), "緊急停止の後の新しいログイン");
    } finally {
      await server.stop();
    }
  },
  300_000,
);

inv("inv_key_03", "暗号化の鍵を新しいバージョンに入れ替えた後も、保存済みの秘密を使える", async () => {
  const instance = await driver.createIsolatedInstance();
  const receiver = await startReceiver();
  let server = await started(instance.config);
  let member;
  let webhookSecret: string;
  let workspaceId: string;
  let webhookId: string;
  try {
    // 暗号化して保存される秘密を作る: TOTPの秘密と、Webhookの署名用の秘密
    const yomi = server.client!;
    const sa = await yomi.login(instance.serverAdmin);
    await yomi.reauthenticate(sa, instance.serverAdmin);
    workspaceId = (await yomi.createWorkspace(sa, "inv-key-03")).id;
    member = await yomi.createMember(sa, workspaceId, "admin"); // TOTPあり
    const memberSession = await yomi.login(member);
    await yomi.reauthenticate(memberSession, member);
    const hook = await yomi.createWebhook(memberSession, workspaceId, receiver.url(env.harnessAllowedHost, "/key03"));
    webhookSecret = hook.secret;
    webhookId = hook.id;
  } finally {
    await server.stop();
  }

  // 新しい鍵を加えて再暗号化し、古い鍵を外した設定で起動する
  const rotated = await driver.rotateEncryptionKey(instance.config);
  expect(rotated.result.exitCode, `鍵の入れ替えのCLIが失敗した:\n${rotated.result.output}`).toBe(0);
  expect(rotated.config.encryptionKeyFile, "入れ替えた後も、古い鍵の設定のまま").not.toBe(instance.config.encryptionKeyFile);

  server = await started(rotated.config);
  try {
    const yomi = server.client!;
    // TOTPの秘密を使える: 多要素認証でログインできる
    const session = await yomi.login(member!);
    expectSuccess(await yomi.meRaw(asSession(session)), "鍵の入れ替えの後の、多要素認証でのログイン");

    // Webhookの署名用の秘密を使える: テスト送信の署名を、元の秘密で検証できる
    await yomi.reauthenticate(session, member!);
    await yomi.testWebhookRaw(session, workspaceId!, webhookId!);
    const req = await receiver.waitFor((r) => r.path.startsWith("/key03"));
    const signing = driver.webhook.signing(req.headers, req.body);
    const expected = createHmac("sha256", signing.keyBytes(webhookSecret!)).update(signing.signingInput).digest(signing.encoding);
    expect(signing.signatures, "鍵の入れ替えの後のWebhookの署名が、元の秘密で検証できない").toContain(expected);
  } finally {
    await server.stop();
    await receiver.close();
  }
}, 300_000);
