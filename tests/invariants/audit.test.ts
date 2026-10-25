// 監査ログと通知の約束: INV-AUD-01, 02, 03, 04, 05, 06, 07

import { randomBytes } from "node:crypto";
import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asSession, driver, type AuditActionKind, type AuditEntry, type Session } from "./support/driver.js";
import { expectRejected, expectSuccess, expectTextNotLeaked } from "./support/assert.js";
import { simpleBody } from "./support/body.js";
import { env } from "./support/env.js";
import { mailCursor, waitForNewMail } from "./support/mail.js";
import { startReceiver } from "./support/net.js";
import { operationsOf } from "./support/operations.js";
import { cloneTemplate } from "./support/template.js";
import { disposableTarget, lazyWorld, marker, sleep, uniqueIp, type World } from "./support/world.js";

const world = lazyWorld();

/** 版の本文に目印を含む版のID。 */
async function versionWith(w: World, session: Session, workspaceId: string, entryId: string, mark: string): Promise<string> {
  for (const v of await w.cms.listVersions(session, workspaceId, entryId)) {
    if ((await w.cms.getVersionRaw(asSession(session), workspaceId, entryId, v.id)).text.includes(mark)) return v.id;
  }
  throw new Error(`目印 ${mark} を含む版が見つからない`);
}

inv("inv_aud_01", "監査ログには、記事の本文や訪問者の質問文が含まれない", async () => {
  const w = await world();
  const { cms, a } = w;
  const titleMarker = marker("aud01-title");
  const bodyMarker = marker("aud01-body");
  const correctedMarker = marker("aud01-corrected");

  // 記事の作成・公開・訂正・抹消・翻訳（内容を含む操作）
  const e = await cms.createEntry(a.editorSession, a.id, { title: titleMarker, body: simpleBody(bodyMarker) });
  await cms.publishEntry(a.editorSession, a.id, e.id);
  await cms.saveDraft(a.editorSession, a.id, e.id, { body: simpleBody(correctedMarker) });
  await cms.publishEntry(a.editorSession, a.id, e.id, { correction: true });
  await cms.reauthenticate(a.adminSession, a.admin);
  await cms.eraseVersionRaw(a.adminSession, a.id, e.id, await versionWith(w, a.adminSession, a.id, e.id, bodyMarker), []);
  await cms.createTranslation(a.editorSession, a.id, e.id, "en");
  // 訪問者の質問文（チャットボット、M4）は、M4で機能が入ったときにここへ足す

  const logs = [
    JSON.stringify((await cms.readAuditLog(a.adminSession, a.id)).map((x) => x.raw)),
    JSON.stringify((await cms.readAuditLog(w.serverAdminSession, null)).map((x) => x.raw)),
  ];
  const listAuditLog = operationsOf(cms).find((o) => o.name === "listAuditLog")!;
  logs.push((await listAuditLog.call(asSession(a.adminSession), { workspaceId: a.id, entryId: e.id, userId: a.editor.userId, apiKeyId: a.readKey.id })).text);
  for (const log of logs) {
    for (const m of [bodyMarker, correctedMarker]) expectTextNotLeaked(log, m, "監査ログ（記事の本文）");
    expectTextNotLeaked(log, titleMarker, "監査ログ（記事のタイトル）");
  }
  // 対照: 監査ログに、この記事の操作が記録されている（IDは残る）
  expect(logs[0]!.includes(e.id), "監査ログに記事のIDがない").toBe(true);
});

inv("inv_aud_02", "監査ログは、管理画面や管理APIから編集・削除できない", async () => {
  const w = await world();
  const { cms, a } = w;
  const before = await cms.readAuditLog(a.adminSession, a.id);
  const instanceBefore = await cms.readAuditLog(w.serverAdminSession, null);
  expect(before.length, "比べるための監査ログがない").toBeGreaterThan(0);

  // 管理APIの状態を変える操作を、使い捨ての管理者とサーバー管理者ですべて実行する（ログアウトは最後）
  const target = await disposableTarget(w, a);
  const throwaway = await cms.createMember(a.adminSession, a.id, "admin");
  const throwawaySession = await cms.login(throwaway);
  await cms.reauthenticate(throwawaySession, throwaway);
  const ops = operationsOf(cms).filter((o) => o.mutating);
  const ordered = [...ops.filter((o) => o.name !== "logout"), ...ops.filter((o) => o.name === "logout")];
  for (const op of ordered) {
    await op.call(asSession(throwawaySession), target);
  }
  const sa = await cms.createServerAdmin({ mfa: true });
  const saSession = await cms.login(sa);
  await cms.reauthenticate(saSession, sa);
  for (const op of ordered) {
    await op.call(asSession(saSession), target);
  }

  // 前からある記録は、1件も消えず、1件も変わっていない
  const check = (beforeEntries: AuditEntry[], afterEntries: AuditEntry[], label: string) => {
    const byId = new Map(afterEntries.map((x) => [x.id, x]));
    for (const old of beforeEntries) {
      const now = byId.get(old.id);
      expect(now, `${label}の記録 ${old.id} が消えた`).toBeDefined();
      expect(now!.raw, `${label}の記録 ${old.id} が変わった`).toEqual(old.raw);
    }
  };
  check(before, await cms.readAuditLog(a.adminSession, a.id), "ワークスペースの監査ログ");
  check(instanceBefore, await cms.readAuditLog(w.serverAdminSession, null), "インスタンスの監査ログ");
}, 180_000);

inv("inv_aud_03", "ワークスペースの管理者は、他のワークスペースやインスタンス全体の監査ログを見られない", async () => {
  const w = await world();
  const { cms, a, b } = w;
  const names = driver.auditActions();
  const rc = await startReceiver();
  try {
    // A の書き出し先（Webhook）を先に作っておく
    await cms.reauthenticate(a.adminSession, a.admin);
    await cms.createAuditSink(a.adminSession, a.id, rc.url(env.harnessAllowedHost, "/aud03"));

    // B の記録（APIキーの発行）と、インスタンス全体だけの記録（ワークスペースの作成）と、A の記録（対照）
    await cms.reauthenticate(b.adminSession, b.admin);
    const bKey = await cms.createApiKey(b.adminSession, b.id, ["read"]);
    await cms.reauthenticate(w.serverAdminSession, w.serverAdmin);
    const instanceOnly = await cms.createWorkspace(w.serverAdminSession, `inv-aud03-${marker("ws").slice(-8)}`);
    await cms.reauthenticate(a.adminSession, a.admin);
    const aKey = await cms.createApiKey(a.adminSession, a.id, ["read"]);
    const hidden = [bKey.id, instanceOnly.id];

    // B の記録とインスタンス全体の記録は、閲覧・絞り込み・ダウンロードのどれでも読めない
    const asA = asSession(a.adminSession);
    const filter = { action: names.apiKeyIssued };
    expectRejected(await cms.auditLogRaw(asA, b.id), "A の管理者による、B の監査ログの閲覧");
    expectRejected(await cms.auditLogRaw(asA, b.id, filter), "A の管理者による、B の監査ログの絞り込み");
    expectRejected(await cms.exportAuditLogRaw(asA, b.id), "A の管理者による、B の監査ログのダウンロード");
    expectRejected(await cms.auditLogRaw(asA, null), "A の管理者による、インスタンス全体の監査ログの閲覧");
    expectRejected(await cms.exportAuditLogRaw(asA, null), "A の管理者による、インスタンス全体の監査ログのダウンロード");

    // A の閲覧・絞り込み・ダウンロードには、A の記録だけがある
    const own = [
      ["閲覧", await cms.auditLogRaw(asA, a.id)],
      ["絞り込み", await cms.auditLogRaw(asA, a.id, filter)],
      ["ダウンロード", await cms.exportAuditLogRaw(asA, a.id)],
    ] as const;
    for (const [label, res] of own) {
      expectSuccess(res, `A の監査ログの${label}`);
      expect(res.text.includes(aKey.id), `A の監査ログの${label}に、A の記録がない`).toBe(true);
      for (const id of hidden) expectTextNotLeaked(res.text, id, `A の監査ログの${label}（B やインスタンス全体の記録）`);
    }

    // A の書き出し先には、A の記録だけが届く
    await rc.waitFor((r) => r.path.startsWith("/aud03") && r.body.includes(aKey.id), 60_000);
    const delivered = rc.requests.filter((r) => r.path.startsWith("/aud03")).map((r) => r.body).join("\n");
    for (const id of hidden) expectTextNotLeaked(delivered, id, "A の監査ログの書き出し先に届いたもの（B やインスタンス全体の記録）");
  } finally {
    await rc.close();
  }
}, 120_000);

inv(
  "inv_aud_04",
  "重い操作（権限の変更、APIキーの発行と失効、予算の変更、質問ログの閲覧、編集の引き継ぎ、訂正としての公開、版の本文の抹消など）を行うと、監査ログに記録される。対象の操作が実装されるたびに、このテストの検査範囲を広げる",
  async () => {
    const w = await world();
    const { cms, a } = w;
    const names = driver.auditActions();
    const admin = a.adminSession;
    const logged = async (kind: AuditActionKind, id: string) => {
      const entries = [...(await cms.readAuditLog(admin, a.id)), ...(await cms.readAuditLog(w.serverAdminSession, null))];
      const hit = entries.some((x) => x.action === names[kind] && (x.targetIds.includes(id) || x.actorUserId === id));
      expect(hit, `${kind}（${names[kind]}）が監査ログに記録されていない（対象 ${id}）`).toBe(true);
    };

    // M1で実装される重い操作
    const member = await cms.createMember(admin, a.id, "editor");
    await cms.reauthenticate(admin, a.admin);
    await cms.changeMemberRole(admin, a.id, member.userId, "author");
    await logged("roleChanged", member.userId);

    const key = await cms.createApiKey(admin, a.id, ["read"]);
    await logged("apiKeyIssued", key.id);
    await cms.revokeApiKey(admin, a.id, key.id);
    await logged("apiKeyRevoked", key.id);

    const token = await cms.createPersonalToken(admin, [a.id], { scopes: ["read"] });
    await logged("personalTokenIssued", token.id);
    await cms.revokePersonalToken(admin, token.id);
    await logged("personalTokenRevoked", token.id);

    const v1 = marker("aud04-v1");
    const e = await cms.createEntry(a.editorSession, a.id, { title: "監査", body: simpleBody(v1) });
    await cms.publishEntry(a.editorSession, a.id, e.id);
    await cms.saveDraft(a.editorSession, a.id, e.id, { body: simpleBody(marker("aud04-v2")) });
    await cms.publishEntry(a.editorSession, a.id, e.id, { correction: true });
    await logged("correctionPublished", e.id);

    await cms.reauthenticate(admin, a.admin);
    expectSuccess(await cms.eraseVersionRaw(admin, a.id, e.id, await versionWith(w, admin, a.id, e.id, v1), []), "版の抹消");
    await logged("versionErased", e.id);

    expectSuccess(await cms.issuePasswordResetRaw(admin, a.id, member.userId), "リセットの発行");
    await logged("passwordResetIssued", member.userId);
    expectSuccess(await cms.issueMfaResetRaw(admin, a.id, member.userId), "多要素認証のリセットの発行");
    await logged("mfaResetIssued", member.userId);

    await cms.login(member, { extras: { clientIp: uniqueIp() } });
    await logged("login", member.userId);

    const deployToken = await cms.deployToken(w.serverAdminSession, w.serverAdmin);
    const template = cloneTemplate(`inv-aud-04-${randomBytes(4).toString("hex")}`);
    expectSuccess(await cms.deployTemplateRaw({ kind: "bearer", token: deployToken }, template), "デプロイ");
    const sa = await cms.me(w.serverAdminSession);
    await logged("schemaDeployed", sa.userId);
    // 予算の変更・質問ログの閲覧（M4）、編集の引き継ぎ（M2）は、実装されたときにここへ足す
  },
  180_000,
);

inv("inv_aud_05", "サーバー管理者がワークスペースのデータを操作すると、そのワークスペースの監査ログに記録される", async () => {
  const w = await world();
  const { cms, a } = w;
  const sa = await cms.me(w.serverAdminSession);
  const member = await cms.createMember(a.adminSession, a.id, "editor");
  const before = new Set((await cms.readAuditLog(a.adminSession, a.id)).map((x) => x.id));

  await cms.reauthenticate(w.serverAdminSession, w.serverAdmin);
  await cms.changeMemberRole(w.serverAdminSession, a.id, member.userId, "author");
  expectSuccess(await cms.issuePasswordResetRaw(w.serverAdminSession, null, member.userId), "サーバー管理者によるリセットの発行");

  // ワークスペースの管理者が見られる、そのワークスペースの監査ログに残る
  const fresh = (await cms.readAuditLog(a.adminSession, a.id)).filter((x) => !before.has(x.id));
  const bySa = fresh.filter((x) => x.actorUserId === sa.userId);
  const names = driver.auditActions();
  expect(bySa.some((x) => x.action === names.roleChanged && x.targetIds.includes(member.userId)), "サーバー管理者によるロールの変更が、ワークスペースの監査ログにない").toBe(true);
  expect(bySa.some((x) => x.action === names.passwordResetIssued && x.targetIds.includes(member.userId)), "サーバー管理者によるリセットの発行が、ワークスペースの監査ログにない").toBe(true);
});

inv(
  "inv_aud_06",
  "新しい端末からのログイン、パスワードや多要素認証の変更、APIキーや個人トークンの発行があると、SMTPを設定している場合は本人に通知が送られ、その通知に秘密の値は含まれない",
  async () => {
    const { cms, a } = await world();
    const user = await cms.createMember(a.adminSession, a.id, "editor");
    const secrets: string[] = [user.password];
    const expectNotice = async (label: string, to: string, action: () => Promise<void>) => {
      const seen = await mailCursor(to);
      await action();
      const mail = await waitForNewMail(to, seen);
      for (const s of secrets) expectTextNotLeaked(mail.raw, s, `${label}の通知`);
    };

    // 新しい端末からのログイン（端末のCookieを持たない）
    let session = await cms.login(user);
    for (const v of session.cookies.values()) secrets.push(v);
    await expectNotice("新しい端末からのログイン", user.email, async () => {
      const s = await cms.login(user, { extras: { clientIp: uniqueIp() } });
      for (const v of s.cookies.values()) secrets.push(v);
    });

    // パスワードの変更
    const newPassword = `inv-${randomBytes(18).toString("base64url")}`;
    secrets.push(newPassword);
    await expectNotice("パスワードの変更", user.email, () => cms.changePassword(session, user, newPassword));
    const changed = { ...user, password: newPassword };
    session = await cms.login(changed);

    // 多要素認証の変更（設定）
    let totpSecret = "";
    await expectNotice("多要素認証の設定", user.email, async () => {
      totpSecret = await cms.enrollTotp(session, changed);
      secrets.push(totpSecret);
    });
    const withMfa = { ...changed, totpSecret };
    session = await cms.login(withMfa);
    await cms.reauthenticate(session, withMfa);

    // 個人トークンの発行（本人へ）
    await expectNotice("個人トークンの発行", user.email, async () => {
      secrets.push((await cms.createPersonalToken(session, [a.id], { scopes: ["read"] })).secret);
    });

    // APIキーの発行（発行した管理者へ）
    await cms.reauthenticate(a.adminSession, a.admin);
    await expectNotice("APIキーの発行", a.admin.email, async () => {
      secrets.push((await cms.createApiKey(a.adminSession, a.id, ["read"])).secret);
    });
  },
  180_000,
);

inv("inv_aud_07", "ログインの履歴（IPアドレス、ブラウザの種類、国）は、保存期間を過ぎると削除される", async () => {
  const { a } = await world();
  const server = await driver.startServer({
    ...(await driver.validServerConfig()),
    loginHistoryRetention: { retentionSeconds: 3, sweepIntervalSeconds: 1 },
  });
  try {
    expect(server.outcome, `ログインの履歴の保存期間を短くしたサーバーが起動しない:\n${server.output}`).toBe("ready");
    const cms = server.client!;
    const ip = uniqueIp();
    await cms.login(a.editor, { extras: { clientIp: ip } });
    const session = await cms.login(a.editor);
    const history = await cms.loginHistoryRaw(session);
    expectSuccess(history, "ログインの履歴の取得");
    expect(history.text.includes(ip), "ログインの履歴に接続元のIPアドレスがない").toBe(true);

    await sleep(8);
    const fresh = await cms.login(a.editor);
    const after = await cms.loginHistoryRaw(fresh);
    expectSuccess(after, "保存期間の後のログインの履歴の取得");
    expect(after.text.includes(ip), "保存期間を過ぎたログインの履歴が残っている").toBe(false);
  } finally {
    await server.stop();
  }
}, 120_000);
