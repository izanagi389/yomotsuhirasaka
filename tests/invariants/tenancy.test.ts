// ワークスペースの分離の約束: INV-TEN-01, 02, 03, 04, 05, 06

import { randomUUID } from "node:crypto";
import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asSession, driver } from "./support/driver.js";
import { expectNotLeaked, expectRateLimited, expectRejected, expectSuccess } from "./support/assert.js";
import { expectIndistinguishable } from "./support/compare.js";
import { mailCursor, waitForToken } from "./support/mail.js";
import { operationsOf } from "./support/operations.js";
import { disposableTarget, lazyWorld, uniqueEmail } from "./support/world.js";

const world = lazyWorld();

inv("inv_ten_01", "あるワークスペースのAPIキーでは、他のワークスペースのコンテンツを取得できない", async () => {
  const { cms, a, b } = await world();

  // 対照: 自分のワークスペースのコンテンツは取得できる
  const own = await cms.deliveryRaw(a.id, a.readKey.secret, a.listQuery);
  expectSuccess(own, "Aのキーで、AのURLのAの記事の一覧");
  expect(own.text.includes(a.secretMarker), "Aの記事の一覧にAの記事が含まれていない").toBe(true);

  // AのキーでBのURLを呼ぶ（Bで登録したクエリ・Aで登録したクエリ）
  for (const [what, query] of [["Bで登録したクエリ", b.listQuery], ["Aで登録したクエリ", a.listQuery]] as const) {
    const res = await cms.deliveryRaw(b.id, a.readKey.secret, query);
    expectRejected(res, `Aのキーで、BのURLの${what}`);
    expectNotLeaked(res, b.secretMarker, `Aのキーで、BのURLの${what}`);
  }

  // AのキーとAのURLで、Bの記事のIDを指定する
  const byId = await cms.deliveryRaw(a.id, a.readKey.secret, a.byIdQuery, { id: b.entryId });
  expectNotLeaked(byId, b.secretMarker, "Aのキーで、AのURLからBの記事のIDを指定");
});

inv("inv_ten_02", "あるワークスペースの管理画面の利用者は、所属していない他のワークスペースのデータを読み書きできない", async () => {
  const w = await world();
  const { cms, a, b } = w;
  const targetB = await disposableTarget(w, b);
  await cms.reauthenticate(a.adminSession, a.admin);
  const aAdmin = asSession(a.adminSession);

  // 対照: 自分のワークスペースの記事は読める
  const own = await cms.getEntryRaw(aAdmin, a.id, a.entryId);
  expectSuccess(own, "Aの管理者によるAの記事の取得");
  expect(own.text.includes(a.secretMarker)).toBe(true);

  // Bの記事を、BのURLでもAのURLでも読めない
  const direct = await cms.getEntryRaw(aAdmin, b.id, b.entryId);
  expectRejected(direct, "Aの管理者によるBの記事の取得");
  expectNotLeaked(direct, b.secretMarker, "Aの管理者によるBの記事の取得");
  expectNotLeaked(await cms.getEntryRaw(aAdmin, a.id, b.entryId), b.secretMarker, "Aの管理者が、AのURLでBの記事のIDを指定");

  // Bに向けたすべての操作（ワークスペースに対する操作）が拒否され、Bのデータが漏れない
  // （本人に対する操作は対象を持たず、インスタンスに対する操作は INV-TEN-06 で確かめる）
  for (const op of operationsOf(cms).filter((o) => o.scope === "workspace")) {
    const res = await op.call(aAdmin, targetB);
    expectRejected(res, `Aの管理者によるBへの ${op.name}`);
    expectNotLeaked(res, b.secretMarker, `Aの管理者によるBへの ${op.name}`);
  }

  // Bのデータが書き換わっていない（舞台の記事・キー・利用者が元のまま）
  const after = await cms.getEntryRaw(asSession(b.adminSession), b.id, b.entryId);
  expectSuccess(after, "Bの管理者によるBの記事の取得");
  expect(after.text.includes(b.secretMarker), "Bの記事が書き換えられている").toBe(true);
  expectSuccess(await cms.deliveryRaw(b.id, b.readKey.secret, b.listQuery), "Bのキーによる配信");
  expectSuccess(await cms.meRaw(asSession(b.editorSession)), "Bの編集者の本人情報の取得");
});

inv("inv_ten_06", "サーバー管理者以外は、ワークスペースを作成できない", async () => {
  const { cms, serverAdmin, serverAdminSession, a } = await world();

  await cms.reauthenticate(a.adminSession, a.admin);
  expectRejected(await cms.createWorkspaceRaw(asSession(a.adminSession), "inv-ten-06-admin"), "ワークスペースの管理者による作成");
  expectRejected(await cms.createWorkspaceRaw(asSession(a.editorSession), "inv-ten-06-editor"), "編集者による作成");

  const token = await cms.createPersonalToken(a.adminSession, [a.id]);
  expectRejected(await cms.createWorkspaceRaw({ kind: "bearer", token: token.secret }, "inv-ten-06-token"), "ワークスペースの管理者の個人トークンによる作成");

  // 対照: サーバー管理者は作成できる
  await cms.reauthenticate(serverAdminSession, serverAdmin);
  expectSuccess(await cms.createWorkspaceRaw(asSession(serverAdminSession), "inv-ten-06-server-admin"), "サーバー管理者による作成");
});

inv("inv_ten_03", "あるワークスペースが回数制限の上限に達しても、他のワークスペースのリクエストは制限されない", async () => {
  const { a, b } = await world();
  const server = await driver.startServer({ ...(await driver.validServerConfig()), rateLimits: { deliveryPerWorkspace: 10 } });
  try {
    expect(server.outcome, `回数制限を小さくしたサーバーが起動しない:\n${server.output}`).toBe("ready");
    const cms = server.client!;

    // Aを上限まで使い切る
    let limited = false;
    for (let i = 0; i < 40 && !limited; i++) {
      const res = await cms.deliveryRaw(a.id, a.readKey.secret, a.listQuery);
      if (res.status === 429) limited = true;
    }
    expect(limited, "Aへのリクエストが回数制限で止められない").toBe(true);
    expectRateLimited(await cms.deliveryRaw(a.id, a.readKey.secret, a.listQuery), "上限に達した後のAへのリクエスト");

    // Bは制限されない
    for (let i = 0; i < 3; i++) {
      expectSuccess(await cms.deliveryRaw(b.id, b.readKey.secret, b.listQuery), "Aが上限に達した後のBへのリクエスト");
    }
  } finally {
    await server.stop();
  }
}, 120_000);

inv("inv_ten_04", "ワークスペースの管理者は、自分のワークスペースに所属していない利用者の一覧や存在を、管理APIから知ることができない", async () => {
  const w = await world();
  const { cms, a, b } = w;
  const outsider = await cms.createMember(b.adminSession, b.id, "editor"); // Bにだけ所属
  await cms.reauthenticate(a.adminSession, a.admin);
  const aAdmin = asSession(a.adminSession);

  // Aに向けた読み取りの操作（対象の利用者に outsider を指定）に、outsider のメールアドレスが出てこない
  const target = { ...(await disposableTarget(w, a)), userId: outsider.userId };
  for (const op of operationsOf(cms).filter((o) => !o.mutating && o.scope !== "instance")) {
    const res = await op.call(aAdmin, target);
    expectNotLeaked(res, outsider.email, `Aの管理者の ${op.name}`);
  }

  // 招待の応答は、相手が既存の利用者かどうかで変わらない
  const existing = await cms.inviteRaw(a.adminSession, a.id, outsider.email, "editor");
  const fresh = await cms.inviteRaw(a.adminSession, a.id, uniqueEmail("ten04-new"), "editor");
  expectIndistinguishable(existing, fresh, "既存の利用者と新しいメールアドレスへの招待");

  // リセットの発行の応答は、所属していない既存の利用者と、存在しない利用者とで変わらない
  const resetOutsider = await cms.issuePasswordResetRaw(a.adminSession, a.id, outsider.userId);
  const resetNobody = await cms.issuePasswordResetRaw(a.adminSession, a.id, randomUUID());
  expectRejected(resetOutsider, "所属していない利用者のリセットの発行");
  expectIndistinguishable(resetOutsider, resetNobody, "所属していない既存の利用者と、存在しない利用者のリセットの発行");
});

inv("inv_ten_05", "招待された既存の利用者は、本人が承諾するまでそのワークスペースに所属しない", async () => {
  const { cms, a, b } = await world();
  const user = await cms.createMember(b.adminSession, b.id, "editor"); // Bにだけ所属
  const session = await cms.login(user);
  const listWorkspaces = cms.adminOperations.find((o) => o.name === "listWorkspaces")!;
  const target = { workspaceId: a.id, entryId: a.entryId, userId: user.userId, apiKeyId: a.readKey.id };

  const seen = await mailCursor(user.email);
  expectSuccess(await cms.inviteRaw(a.adminSession, a.id, user.email, "editor"), "既存の利用者のAへの招待");
  const token = await waitForToken(user.email, seen, "invite");

  // 承諾する前: Aは一覧に出ず、Aのデータも読めない
  const before = await listWorkspaces.call(asSession(session), target);
  expectSuccess(before, "承諾前のワークスペースの一覧");
  expect(before.text.includes(a.id), "承諾前なのに、ワークスペースの一覧にAがある").toBe(false);
  const read = await cms.getEntryRaw(asSession(session), a.id, a.entryId);
  expectRejected(read, "承諾前のAの記事の取得");
  expectNotLeaked(read, a.secretMarker, "承諾前のAの記事の取得");

  // 対照: 承諾すれば所属する
  expectSuccess(await cms.acceptInviteRaw(token, { session }), "招待の承諾");
  const after = await listWorkspaces.call(asSession(session), target);
  expect(after.text.includes(a.id), "承諾した後も、ワークスペースの一覧にAがない").toBe(true);
  expectSuccess(await cms.getEntryRaw(asSession(session), a.id, a.entryId), "承諾後のAの記事の取得");
});
