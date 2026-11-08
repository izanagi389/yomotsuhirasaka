// パスワードと多要素認証の回復、招待の約束: INV-AUTH-10, 14, 15, 16

import { expect } from "vitest";
import { randomBytes } from "node:crypto";
import { inv } from "./support/inv.js";
import { asSession, driver } from "./support/driver.js";
import { expectRefused, expectRejected, expectSuccess } from "./support/assert.js";
import { expectIndistinguishable } from "./support/compare.js";
import { isSuccess } from "./support/http.js";
import { mailCursor, newMailWithin, waitForToken } from "./support/mail.js";
import { lazyWorld, sleep, uniqueEmail, uniqueIp } from "./support/world.js";

const world = lazyWorld();
const strongPassword = (): string => `inv-${randomBytes(18).toString("base64url")}`;

inv("inv_auth_10", "サーバー管理者のパスワードと多要素認証は、管理APIからはリセットできない", async () => {
  const { yomi, a } = await world();
  const target = await yomi.createServerAdmin({ mfa: true });
  const other = await yomi.createServerAdmin({ mfa: true });
  const otherSession = await yomi.login(other);
  await yomi.reauthenticate(otherSession, other);

  // 別のサーバー管理者からも、リセットを発行できない
  expectRejected(await yomi.issuePasswordResetRaw(otherSession, null, target.userId), "サーバー管理者によるサーバー管理者のパスワードのリセット");
  expectRejected(await yomi.issueMfaResetRaw(otherSession, null, target.userId), "サーバー管理者によるサーバー管理者の多要素認証のリセット");

  // ワークスペースの管理者からも（対象がそのワークスペースに所属していても）
  await yomi.addExistingMember(a.adminSession, a.id, target, "editor");
  await yomi.reauthenticate(a.adminSession, a.admin);
  expectRejected(await yomi.issuePasswordResetRaw(a.adminSession, a.id, target.userId), "ワークスペースの管理者によるサーバー管理者のパスワードのリセット");
  expectRejected(await yomi.issueMfaResetRaw(a.adminSession, a.id, target.userId), "ワークスペースの管理者によるサーバー管理者の多要素認証のリセット");

  // 本人がメールでリセットを要求しても、ネットワーク越しには変えられない
  const seen = await mailCursor(target.email);
  await yomi.requestPasswordResetRaw(target.email, { clientIp: uniqueIp() });
  const mails = await newMailWithin(target.email, seen, 5_000);
  for (const m of mails) {
    const token = driver.mail.tokenFrom(m, "reset");
    if (token) {
      const res = await yomi.completePasswordResetRaw(token, strongPassword());
      expect(isSuccess(res), "サーバー管理者のパスワードをメールのリンクからリセットできてしまった").toBe(false);
    }
  }
  // 元のパスワードと多要素認証のままログインできる
  expectSuccess(await yomi.meRaw(asSession(await yomi.login(target))), "リセットを試みられた後のサーバー管理者のログイン");

  // 対照: サーバー管理者は、一般の利用者のリセットは発行できる
  const user = await yomi.createMember(a.adminSession, a.id, "editor");
  expectSuccess(await yomi.issuePasswordResetRaw(otherSession, null, user.userId), "サーバー管理者による一般の利用者のリセット");
});

inv(
  "inv_auth_14",
  "ワークスペースの管理者は、自分が管理者ではないワークスペースにも所属する利用者のパスワードや多要素認証のリセットを発行できない",
  async () => {
    const { yomi, a, b } = await world();
    const onlyA = await yomi.createMember(a.adminSession, a.id, "editor");
    const alsoB = await yomi.createMember(a.adminSession, a.id, "editor");
    await yomi.addExistingMember(b.adminSession, b.id, alsoB, "viewer");
    await yomi.reauthenticate(a.adminSession, a.admin);

    expectRejected(await yomi.issuePasswordResetRaw(a.adminSession, a.id, alsoB.userId), "Bにも所属する利用者のパスワードのリセット");
    expectRejected(await yomi.issueMfaResetRaw(a.adminSession, a.id, alsoB.userId), "Bにも所属する利用者の多要素認証のリセット");

    // 対照: Aにしか所属していない利用者には発行できる
    expectSuccess(await yomi.issuePasswordResetRaw(a.adminSession, a.id, onlyA.userId), "Aにしか所属していない利用者のパスワードのリセット");
    expectSuccess(await yomi.issueMfaResetRaw(a.adminSession, a.id, onlyA.userId), "Aにしか所属していない利用者の多要素認証のリセット");
  },
);

inv("inv_auth_15", "一度使ったリセット用・招待用のトークンと、有効期限の切れたものは使えない", async () => {
  const { yomi, a } = await world();

  // リセット用のトークン: 2回目は使えない
  const user = await yomi.createMember(a.adminSession, a.id, "editor");
  let seen = await mailCursor(user.email);
  await yomi.requestPasswordResetRaw(user.email, { clientIp: uniqueIp() });
  const resetToken = await waitForToken(user.email, seen, "reset");
  const first = strongPassword();
  expectSuccess(await yomi.completePasswordResetRaw(resetToken, first), "リセット用のトークンの1回目");
  expectRefused(await yomi.completePasswordResetRaw(resetToken, strongPassword()), "リセット用のトークンの2回目");
  expectSuccess(await yomi.meRaw(asSession(await yomi.login({ ...user, password: first }))), "1回目で決めたパスワードでのログイン");

  // 招待用のトークン: 2回目は使えない
  const email = uniqueEmail("auth15");
  seen = await mailCursor(email);
  expectSuccess(await yomi.inviteRaw(a.adminSession, a.id, email, "editor"), "招待");
  const inviteToken = await waitForToken(email, seen, "invite");
  expectSuccess(await yomi.acceptInviteRaw(inviteToken, { newPassword: strongPassword() }), "招待用のトークンの1回目");
  expectRefused(await yomi.acceptInviteRaw(inviteToken, { newPassword: strongPassword() }), "招待用のトークンの2回目");

  // 有効期限の切れたトークン: 有効期限を2秒にしたサーバーで確かめる
  const server = await driver.startServer({ ...(await driver.validServerConfig()), oneTimeTokenLifetimeSeconds: 2 });
  try {
    expect(server.outcome, `トークンの有効期限を短くしたサーバーが起動しない:\n${server.output}`).toBe("ready");
    const short = server.client!;
    const user2 = await yomi.createMember(a.adminSession, a.id, "editor");
    seen = await mailCursor(user2.email);
    await short.requestPasswordResetRaw(user2.email, { clientIp: uniqueIp() });
    const expiredReset = await waitForToken(user2.email, seen, "reset");
    const adminOnShort = await short.login(a.admin);
    const email2 = uniqueEmail("auth15exp");
    const seen2 = await mailCursor(email2);
    expectSuccess(await short.inviteRaw(adminOnShort, a.id, email2, "editor"), "有効期限の短いサーバーでの招待");
    const expiredInvite = await waitForToken(email2, seen2, "invite");

    await sleep(4);
    expectRefused(await short.completePasswordResetRaw(expiredReset, strongPassword()), "有効期限の切れたリセット用のトークン");
    expectRefused(await short.acceptInviteRaw(expiredInvite, { newPassword: strongPassword() }), "有効期限の切れた招待用のトークン");
    expectSuccess(await short.meRaw(asSession(await short.login(user2))), "期限切れのリセットの後も、元のパスワードでログインできる");
  } finally {
    await server.stop();
  }
}, 180_000);

inv("inv_auth_16", "パスワードリセットの要求への応答は、メールアドレスが登録されているかどうかで変わらない", async () => {
  const { yomi, a } = await world();

  for (let i = 0; i < 2; i++) {
    const registered = await yomi.createMember(a.adminSession, a.id, "editor");
    const unregistered = uniqueEmail("auth16-none");
    const seen = await mailCursor(registered.email);

    const r1 = await yomi.requestPasswordResetRaw(registered.email, { clientIp: uniqueIp() });
    const r2 = await yomi.requestPasswordResetRaw(unregistered, { clientIp: uniqueIp() });
    expectIndistinguishable(r1, r2, "登録済みと未登録のメールアドレスへのリセットの要求");

    // 対照: 登録済みのアドレスには、実際にメールが届いている
    await waitForToken(registered.email, seen, "reset");
  }
});
