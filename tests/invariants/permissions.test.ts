// 重い権限の約束: INV-PERM-02, INV-PERM-03
//
// 重い権限は、ロールの既定に加えて、管理者が個人に追加で付与できる（11-permissions-and-audit.md、Q76）。
// 所属者が持つ権限は管理APIの所属者の応答で読み、操作できるかどうかは実際に操作して確かめる。
// 拒否の確認が「何でも拒否する」実装で通らないよう、付与したら操作できることも対照として確かめる。
// 確かめる重い操作は、方針に関わるスイッチ・訂正としての公開・確認済みの記録（M2）と、
// 埋め込みの作成の予算の変更（M3）。質問ログ（M4）とワークスペースのLLMの接続設定（M4）は、入ったときにここへ足す。
// 重い操作には再認証が要るので、拒否を確かめる前に再認証しておく（再認証の不足で断られたのではないことを確かめるため）。

import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asSession, type HeavyPermission, type Session } from "./support/driver.js";
import { expectRejected, expectSuccess } from "./support/assert.js";
import { doc, paragraph } from "./support/body.js";
import { lazyWorld, marker } from "./support/world.js";

const world = lazyWorld();

/** ワークスペースAの管理者として、所属者に付与する重い権限を置き換える（管理者は再認証してから）。 */
async function grant(userId: string, grants: HeavyPermission[]): Promise<void> {
  const { yomi, a } = await world();
  await yomi.reauthenticate(a.adminSession, a.admin);
  expectSuccess(await yomi.setMemberGrantsRaw(asSession(a.adminSession), a.id, userId, grants), "管理者による重い権限の付与");
}

inv("inv_perm_02", "質問ログの閲覧権限は、どのロールにも既定では付与されていない", async () => {
  const { yomi, a } = await world();
  await yomi.reauthenticate(a.adminSession, a.admin);
  const author = await yomi.createMember(a.adminSession, a.id, "author");
  const viewer = await yomi.createMember(a.adminSession, a.id, "viewer");
  const members: [string, string][] = [
    ["管理者", a.admin.userId],
    ["編集者", a.editor.userId],
    ["執筆者", author.userId],
    ["閲覧者", viewer.userId],
  ];
  for (const [role, userId] of members) {
    const permissions = await yomi.memberPermissions(a.adminSession, a.id, userId);
    expect(permissions, `${role}が、付与なしで質問ログの閲覧権限を持っている`).not.toContain("viewQuestionLog");
  }
  // 対照: 権限の一覧にはロールの既定が含まれる（管理者は方針のスイッチを既定で持つ）
  expect(await yomi.memberPermissions(a.adminSession, a.id, a.admin.userId), "管理者の権限の一覧に、ロールの既定が含まれていない").toContain("policySwitches");

  // 管理者でない所属者は、自分にも他人にも付与できない
  expectRejected(await yomi.setMemberGrantsRaw(asSession(a.editorSession), a.id, a.editor.userId, ["viewQuestionLog"]), "編集者が自分に質問ログの閲覧権限を付与する");
  expectRejected(await yomi.setMemberGrantsRaw(asSession(a.editorSession), a.id, viewer.userId, ["viewQuestionLog"]), "編集者が他人に質問ログの閲覧権限を付与する");
  expect(await yomi.memberPermissions(a.adminSession, a.id, a.editor.userId), "拒否されたはずの付与が反映されている").not.toContain("viewQuestionLog");

  // 対照: 管理者が付与すれば持つ
  await grant(viewer.userId, ["viewQuestionLog"]);
  expect(await yomi.memberPermissions(a.adminSession, a.id, viewer.userId), "管理者が付与した質問ログの閲覧権限が反映されていない").toContain("viewQuestionLog");
});

inv("inv_perm_03", "重い権限（LLMの接続設定と予算の変更など）を持たない利用者は、その操作を実行できない", async () => {
  const { yomi, a } = await world();
  await yomi.reauthenticate(a.adminSession, a.admin);
  const author = await yomi.createMember(a.adminSession, a.id, "author");
  const authorSession: Session = await yomi.login(author);
  const { id } = await yomi.createEntry(a.editorSession, a.id, { title: marker("perm03"), body: doc(paragraph("本文")) });
  await yomi.publishEntry(a.editorSession, a.id, id);

  // --- 方針に関わるスイッチ（既定では管理者だけ） ---
  const optOut = { optOutOfAiTraining: true };
  expectRejected(await yomi.setPublishSettingsRaw(asSession(a.editorSession), a.id, id, optOut), "方針のスイッチの権限のない編集者が、学習への利用の拒否を変える");
  expectRejected(await yomi.setPublishSettingsRaw(asSession(authorSession), a.id, id, { includeInAiMaterials: false }), "方針のスイッチの権限のない執筆者が、外部AI向けの素材に含めるを変える");
  expectSuccess(await yomi.setPublishSettingsRaw(asSession(a.adminSession), a.id, id, optOut), "管理者が学習への利用の拒否を変える");
  // 付与すれば操作でき、取り消せばすぐに操作できなくなる（付与は操作のたびに照合する）
  await grant(a.editor.userId, ["policySwitches"]);
  expectSuccess(await yomi.setPublishSettingsRaw(asSession(a.editorSession), a.id, id, { optOutOfAiTraining: false }), "方針のスイッチを付与された編集者が、学習への利用の拒否を変える");
  await grant(a.editor.userId, []);
  expectRejected(await yomi.setPublishSettingsRaw(asSession(a.editorSession), a.id, id, optOut), "方針のスイッチの付与を取り消された編集者が、学習への利用の拒否を変える");

  // --- 訂正としての公開と、確認済みの記録（執筆者は付与が要る） ---
  await yomi.saveDraft(authorSession, a.id, id, { body: doc(paragraph(`直した本文 ${marker("perm03-fix")}`)) });
  expectRejected(await yomi.publishRaw(asSession(authorSession), a.id, id, { correction: true, note: "訂正" }), "訂正の権限のない執筆者が、訂正として公開する");
  expectRejected(await yomi.markVerifiedRaw(asSession(authorSession), a.id, id, "確認"), "訂正の権限のない執筆者が、確認済みを記録する");
  await grant(author.userId, ["corrections"]);
  expectSuccess(await yomi.markVerifiedRaw(asSession(authorSession), a.id, id, "確認"), "訂正の権限を付与された執筆者が、確認済みを記録する");
  // 訂正の権限は、通常の公開の権限ではない（INV-PERM-01）
  expectRejected(await yomi.publishRaw(asSession(authorSession), a.id, id), "訂正の権限を付与された執筆者が、通常の公開をする");
  expectSuccess(await yomi.publishRaw(asSession(authorSession), a.id, id, { correction: true, note: "訂正" }), "訂正の権限を付与された執筆者が、訂正として公開する");
  await grant(author.userId, []);
  expectRejected(await yomi.markVerifiedRaw(asSession(authorSession), a.id, id, "確認"), "訂正の権限の付与を取り消された執筆者が、確認済みを記録する");

  // --- 埋め込みの作成の予算の変更（LLMの接続設定と予算の権限。既定では管理者だけ） ---
  // 値は、サーバー管理者の天井の範囲に必ず収まる 0 にする（天井を超える値として断られたのではないことを確かめるため）
  const budget = { tokensPerMonth: 0, tokensPerDay: 0 };
  await yomi.reauthenticate(a.editorSession, a.editor);
  await yomi.reauthenticate(authorSession, author);
  expectRejected(await yomi.setEmbeddingBudgetRaw(asSession(a.editorSession), a.id, budget), "LLMの権限のない編集者が、埋め込みの予算を変える");
  expectRejected(await yomi.setEmbeddingBudgetRaw(asSession(authorSession), a.id, budget), "LLMの権限のない執筆者が、埋め込みの予算を変える");
  expectSuccess(await yomi.setEmbeddingBudgetRaw(asSession(a.adminSession), a.id, budget), "管理者が埋め込みの予算を変える");
  await grant(a.editor.userId, ["manageLlm"]);
  expectSuccess(await yomi.setEmbeddingBudgetRaw(asSession(a.editorSession), a.id, budget), "LLMの権限を付与された編集者が、埋め込みの予算を変える");
  await grant(a.editor.userId, []);
  expectRejected(await yomi.setEmbeddingBudgetRaw(asSession(a.editorSession), a.id, budget), "LLMの権限の付与を取り消された編集者が、埋め込みの予算を変える");
});
