// 管理APIが返す、所属・招待・キー・Webhook・監査ログの形（server/src/http/admin_*.rs）。

import type { HeavyPermission } from "~/utils/content";

export type Role = "admin" | "editor" | "author" | "viewer";

/** 重い権限を個人に付与できるロール（11-permissions-and-audit.md の表。ロールの既定で持つものは除く）。 */
export const GRANTABLE_TO: Record<HeavyPermission, Role[]> = {
  viewQuestionLog: ["admin", "editor", "author", "viewer"],
  manageLlm: ["editor", "author", "viewer"],
  policySwitches: ["editor", "author"],
  corrections: ["author"],
  takeOverEditing: ["author"],
};
export const ROLES: Role[] = ["admin", "editor", "author", "viewer"];

export interface MemberInfo {
  userId: string;
  email: string;
  role: Role;
  canViewMembers: boolean;
  /** 個人に付与した重い権限。 */
  grants: HeavyPermission[];
  /** 実際に持つ重い権限（ロールの既定と、個人への付与）。 */
  permissions: HeavyPermission[];
  mfaEnabled: boolean;
  joinedAt: string;
}

export interface InvitationSummary {
  id: string;
  email: string;
  role: Role;
  createdAt: string;
  expiresAt: string;
}

/** 招待やリセットの発行の応答。SMTP がなければ、リンクを一度だけ返す（Q193）。 */
export interface Issued {
  expiresAt: string;
  delivery: "email" | "link";
  inviteUrl?: string;
  resetUrl?: string;
}

export interface ApiKeySummary {
  id: string;
  name: string;
  displayPrefix: string;
  scopes: string[];
  purpose: "build" | "server";
  createdAt: string;
  expiresAt: string | null;
  lastUsedAt: string | null;
}

export interface ApiKeyCreated extends Omit<ApiKeySummary, "displayPrefix" | "createdAt" | "lastUsedAt"> {
  /** 秘密の値（この応答でだけ渡される）。 */
  key: string;
}

export interface WebhookSummary {
  id: string;
  url: string;
  createdAt: string;
  previousSecretUntil: string | null;
}

export interface WebhookDelivery {
  id: string;
  eventId: string;
  eventType: string;
  entryId: string | null;
  state: "pending" | "delivered" | "failed";
  attempts: number;
  lastStatus: number | null;
  lastError: string | null;
  createdAt: string;
  lastAttemptAt: string | null;
}

export interface AuditEntry {
  id: string;
  workspaceId: string | null;
  actorUserId: string | null;
  action: string;
  targetIds: string[];
  createdAt: string;
}

export interface AuditPage {
  entries: AuditEntry[];
  nextBefore: string | null;
}

/** ワークスペースの管理の画面を使えるか（管理者とサーバー管理者）。 */
export function isWorkspaceAdmin(w: { role: string | null } | null | undefined): boolean {
  return !!w && (w.role === null || w.role === "admin");
}
