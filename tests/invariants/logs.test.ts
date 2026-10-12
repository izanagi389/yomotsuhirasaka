// ログの約束: INV-LOGS-01
//
// ログをもっとも詳しくしたサーバーを別に起動し、秘密や内容を含む操作をひととおり行ってから、
// 出力されたログに、それらが1つも含まれないことを確かめる。

import { randomBytes } from "node:crypto";
import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asBearer, asSession, driver, type Session } from "./support/driver.js";
import { expectTextNotLeaked } from "./support/assert.js";
import { simpleBody } from "./support/body.js";
import { mailCursor, waitForToken } from "./support/mail.js";
import { lazyWorld, marker, uniqueEmail, uniqueIp } from "./support/world.js";

const world = lazyWorld();

inv("inv_logs_01", "サーバーのログには、パスワード・トークン・APIキー・Cookie・記事の本文・質問文が出力されない", async () => {
  const w = await world();
  const server = await driver.startServer({ ...(await driver.validServerConfig()), logLevel: "trace" });
  const secrets: [string, string][] = [];
  const keep = (label: string, value: string) => secrets.push([label, value]);
  const keepSession = (label: string, s: Session) => {
    for (const [name, value] of s.cookies) keep(`${label}のCookie ${name}`, value);
    if (s.csrfToken) keep(`${label}のCSRFトークン`, s.csrfToken);
  };

  try {
    expect(server.outcome, `ログを詳しくしたサーバーが起動しない:\n${server.output}`).toBe("ready");
    const cms = server.client!;

    // パスワード（成功と失敗）と、多要素認証の秘密
    const user = await w.cms.createMember(w.a.adminSession, w.a.id, "admin");
    keep("パスワード", user.password);
    keep("TOTPの秘密", user.totpSecret!);
    const wrong = `wrong-${randomBytes(12).toString("hex")}`;
    keep("誤ったパスワード", wrong);
    await cms.loginRaw({ ...user, password: wrong }, { extras: { clientIp: uniqueIp() } });
    const session = await cms.login(user);
    keepSession("ログイン", session);
    await cms.reauthenticate(session, user);

    // 記事の本文（保存と公開）
    const bodyMarker = marker("log-body");
    keep("記事の本文", bodyMarker);
    const entry = await cms.createEntry(session, w.a.id, { title: "ログの確認", body: simpleBody(bodyMarker) });
    const draftMarker = marker("log-draft");
    keep("下書きの本文", draftMarker);
    await cms.saveDraft(session, w.a.id, entry.id, { body: simpleBody(draftMarker) });
    await cms.publishEntry(session, w.a.id, entry.id);

    // APIキーと個人トークン（発行と使用）
    const key = await cms.createApiKey(session, w.a.id, ["read"]);
    keep("APIキー", key.secret);
    await cms.deliveryRaw(w.a.id, key.secret, w.a.listQuery);
    await cms.deliveryRaw(w.a.id, `${key.secret}x`, w.a.listQuery); // 誤ったキー
    const token = await cms.createPersonalToken(session, [w.a.id], { scopes: ["read"] });
    keep("個人トークン", token.secret);
    await cms.meRaw(asBearer(token.secret));

    // リセット用・招待用のトークン
    let seen = await mailCursor(user.email);
    await cms.requestPasswordResetRaw(user.email, { clientIp: uniqueIp() });
    keep("リセット用のトークン", await waitForToken(user.email, seen, "reset"));
    const invited = uniqueEmail("logs01");
    seen = await mailCursor(invited);
    await cms.inviteRaw(session, w.a.id, invited, "editor");
    keep("招待用のトークン", await waitForToken(invited, seen, "invite"));

    await cms.meRaw(asSession(session));
    // 質問文（チャットボット、M4）は、M4で機能が入ったときにここへ足す
  } finally {
    await server.stop();
  }

  const log = server.output;
  expect(log.length, "ログが出力されていない（ログの取り込みを確認する）").toBeGreaterThan(0);
  for (const [label, value] of secrets) expectTextNotLeaked(log, value, `サーバーのログ（${label}）`);
}, 180_000);
