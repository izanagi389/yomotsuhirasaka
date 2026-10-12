// 管理APIの操作の一覧（ドライバーの adminOperations）を使うための補助。

import { expect } from "vitest";
import type { AdminOperation, CmsClient } from "./driver.js";

/**
 * M1の管理APIに必ずある操作。ドライバーの一覧にこれらが欠けていたら、テストは失敗する。
 * 一覧はこれより多くてよい（すべての操作を載せる）。
 */
export const REQUIRED_OPERATIONS = {
  // 本人
  me: "self",
  listMySessions: "self",
  revokeMySession: "self",
  changeMyPassword: "self",
  listWorkspaces: "self",
  listMyIdentities: "self",
  getMyLoginHistory: "self",
  logout: "self",
  // インスタンス（サーバー管理者）
  createWorkspace: "instance",
  listUsers: "instance",
  setWorkspaceLimits: "instance",
  deploySchema: "instance",
  // ワークスペースと所属
  getWorkspace: "workspace",
  listMembers: "workspace",
  getMember: "workspace",
  inviteMember: "workspace",
  changeMemberRole: "workspace",
  removeMember: "workspace",
  issuePasswordReset: "workspace",
  issueMfaReset: "workspace",
  getSchemaInfo: "workspace",
  // 記事
  listEntries: "workspace",
  getEntry: "workspace",
  createEntry: "workspace",
  saveDraft: "workspace",
  publishEntry: "workspace",
  publishAsCorrection: "workspace",
  unpublishEntry: "workspace",
  deleteEntry: "workspace",
  changePublishSettings: "workspace",
  listEntryVersions: "workspace",
  getEntryVersion: "workspace",
  eraseVersion: "workspace",
  createTranslation: "workspace",
  getTranslationInfo: "workspace",
  getUrlHistory: "workspace",
  // メディア
  listMedia: "workspace",
  uploadMedia: "workspace",
  deleteMedia: "workspace",
  // Webhook
  listWebhooks: "workspace",
  createWebhook: "workspace",
  testWebhook: "workspace",
  listWebhookDeliveries: "workspace",
  deleteWebhook: "workspace",
  // キーとトークン
  listApiKeys: "workspace",
  createApiKey: "workspace",
  revokeApiKey: "workspace",
  listPersonalTokens: "self",
  createPersonalToken: "self",
  revokePersonalToken: "self",
  // 監査ログ
  listAuditLog: "workspace",
} as const satisfies Record<string, AdminOperation["scope"]>;

/** 操作の一覧を取り出し、必須の操作がそろっていることを確かめる。 */
export function operationsOf(cms: CmsClient): AdminOperation[] {
  const ops = cms.adminOperations;
  const names = new Set(ops.map((o) => o.name));
  const missing = Object.keys(REQUIRED_OPERATIONS).filter((n) => !names.has(n));
  expect(missing, "ドライバーの adminOperations に欠けている操作").toEqual([]);
  for (const op of ops) {
    const expected = (REQUIRED_OPERATIONS as Record<string, string>)[op.name];
    if (expected) expect(op.scope, `${op.name} の向き先`).toBe(expected);
  }
  expect(names.size, "adminOperations に同じ名前の操作が重複している").toBe(ops.length);
  return ops;
}
