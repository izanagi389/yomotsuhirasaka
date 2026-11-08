// 再認証とセッションの約束: INV-AUTH-06, 12, 13
//
// 「再認証が必要か」は、ログインの直後かどうかで変わりうる。そこで、再認証の有効時間を2秒にした
// サーバーを別に起動し、有効時間を過ぎた状態で確かめる。

import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asSession, driver, type YomiClient, type StartedServer } from "./support/driver.js";
import { expectRejected, expectSuccess } from "./support/assert.js";
import { isSuccess } from "./support/http.js";
import { cloneTemplate } from "./support/template.js";
import { disposableTarget, lazyWorld, sleep } from "./support/world.js";

const world = lazyWorld();
const REAUTH_WINDOW = 2;

async function withShortReauthServer(fn: (yomi: YomiClient) => Promise<void>): Promise<void> {
  const server: StartedServer = await driver.startServer({ ...(await driver.validServerConfig()), reauthWindowSeconds: REAUTH_WINDOW });
  try {
    expect(server.outcome, `再認証の有効時間を短くしたサーバーが起動しない:\n${server.output}`).toBe("ready");
    await fn(server.client!);
  } finally {
    await server.stop();
  }
}

inv("inv_auth_06", "重い操作（APIキーの発行、権限の変更など）は、再認証なしでは実行できない", async () => {
  const w = await world();
  const target = await disposableTarget(w, w.a);

  await withShortReauthServer(async (yomi) => {
    const ops = new Map(yomi.adminOperations.map((o) => [o.name, o]));
    const admin = await yomi.login(w.a.admin);
    const serverAdmin = await yomi.login(w.serverAdmin);
    await sleep(REAUTH_WINDOW + 2);

    // 再認証の有効時間を過ぎた状態では、重い操作はすべて拒否される
    const heavy = ["createApiKey", "changeMemberRole", "createPersonalToken", "eraseVersion"];
    for (const name of heavy) {
      expectRejected(await ops.get(name)!.call(asSession(admin), target), `再認証なしの ${name}`);
    }
    expectRejected(
      await yomi.deployTemplateRaw(asSession(serverAdmin), cloneTemplate(`inv-auth-06-${Date.now()}`)),
      "再認証なしのサーバー管理者によるデプロイ",
    );

    // 対照: 再認証すれば実行できる
    await yomi.reauthenticate(admin, w.a.admin);
    for (const name of ["createApiKey", "changeMemberRole", "createPersonalToken"]) {
      expectSuccess(await ops.get(name)!.call(asSession(admin), target), `再認証した後の ${name}`);
    }
  });
}, 180_000);

inv("inv_auth_12", "パスワードや多要素認証を変更すると、その利用者の他のセッションは使えなくなる", async () => {
  const { yomi, a } = await world();
  const user = await yomi.createMember(a.adminSession, a.id, "editor");

  // パスワードの変更
  const s1 = await yomi.login(user);
  const s2 = await yomi.login(user);
  expectSuccess(await yomi.meRaw(asSession(s2)), "変更前の、もう1つのセッション");
  const newPassword = `inv-${Date.now()}-new-password`;
  await yomi.changePassword(s1, user, newPassword);
  expectRejected(await yomi.meRaw(asSession(s2)), "パスワードの変更後の、もう1つのセッション");
  const changed = { ...user, password: newPassword };

  // 多要素認証の変更（設定）
  const s3 = await yomi.login(changed);
  const s4 = await yomi.login(changed);
  expectSuccess(await yomi.meRaw(asSession(s4)), "変更前の、もう1つのセッション");
  await yomi.enrollTotp(s3, changed);
  expectRejected(await yomi.meRaw(asSession(s4)), "多要素認証の設定後の、もう1つのセッション");
});

inv("inv_auth_13", "多要素認証を設定した利用者は、パスワードだけでは再認証を通れない", async () => {
  const w = await world();
  const target = await disposableTarget(w, w.a);

  await withShortReauthServer(async (yomi) => {
    const createApiKey = yomi.adminOperations.find((o) => o.name === "createApiKey")!;
    const session = await yomi.login(w.a.admin); // 多要素認証あり
    await sleep(REAUTH_WINDOW + 2);

    // パスワードだけの再認証は通らず、重い操作もできないまま
    const byPassword = await yomi.reauthenticateRaw(session, { password: w.a.admin.password });
    expect(isSuccess(byPassword), "パスワードだけで再認証を通れてしまった").toBe(false);
    expectRejected(await createApiKey.call(asSession(session), target), "パスワードだけで再認証を試みた後のAPIキーの発行");

    // 対照: 多要素認証のコードなら通る
    expectSuccess(await yomi.reauthenticateRaw(session, { totpCode: await yomi.nextTotpCode(w.a.admin.totpSecret!) }), "多要素認証での再認証");
    expectSuccess(await createApiKey.call(asSession(session), target), "多要素認証で再認証した後のAPIキーの発行");
  });
}, 180_000);
