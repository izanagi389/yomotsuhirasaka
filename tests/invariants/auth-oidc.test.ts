// OIDC/SSOの約束: INV-AUTH-19
//
// テスト環境のCMSには、擬似IdP（support/oidc-idp.ts）を接続先として設定し、
// ドメイン env.oidc.jitDomain のJIT（自動作成）を有効にしておく（README）。

import { expect } from "vitest";
import { randomBytes } from "node:crypto";
import { inv } from "./support/inv.js";
import { asSession } from "./support/driver.js";
import { expectNotLeaked, expectRejected, expectSuccess } from "./support/assert.js";
import { env } from "./support/env.js";
import { startOidcIdp, type MockIdp } from "./support/oidc-idp.js";
import { lazyWorld } from "./support/world.js";

const world = lazyWorld();
const sub = () => `sub-${randomBytes(8).toString("hex")}`;
const jitEmail = (label: string) => `${label}-${randomBytes(6).toString("hex")}@${env.oidc.jitDomain}`;

async function withIdp(fn: (idp: MockIdp) => Promise<void>): Promise<void> {
  const idp = await startOidcIdp({ port: env.oidc.port, issuer: env.oidc.issuer, clientId: env.oidc.clientId, clientSecret: env.oidc.clientSecret });
  try {
    await fn(idp);
  } finally {
    await idp.close();
  }
}

inv(
  "inv_auth_19",
  "OIDCのIDトークンで `email_verified` が確認できない場合、アカウントの作成も既存のアカウントへのひも付けも行われない",
  async () => {
    const w = await world();
    const { cms } = w;
    const listUsers = cms.adminOperations.find((o) => o.name === "listUsers")!;
    const listMyIdentities = cms.adminOperations.find((o) => o.name === "listMyIdentities")!;
    const anyTarget = { workspaceId: w.a.id, entryId: w.a.entryId, userId: w.a.editor.userId, apiKeyId: w.a.readKey.id };

    await withIdp(async (idp) => {
      // JITの対象のドメインでも、email_verified が false・クレームなしならアカウントは作られない
      for (const verified of [false, undefined]) {
        const email = jitEmail("unverified");
        idp.setNextLogin({ sub: sub(), email, email_verified: verified });
        const attempt = await cms.loginWithOidcRaw();
        expectRejected(await cms.meRaw(asSession(attempt.session)), `email_verified=${String(verified)} でのSSOログイン後の本人情報の取得`);
        await cms.reauthenticate(w.serverAdminSession, w.serverAdmin);
        const users = await listUsers.call(asSession(w.serverAdminSession), anyTarget);
        expectSuccess(users, "利用者の一覧");
        expectNotLeaked(users, email, `email_verified=${String(verified)} でのSSOログインの後の利用者の一覧`);
      }

      // 既存のアカウントと同じメールアドレスでも、email_verified が確認できなければひも付けない
      const existing = await cms.createMember(w.a.adminSession, w.a.id, "editor", { email: jitEmail("existing") });
      for (const verified of [false, undefined]) {
        const attackerSub = sub();
        idp.setNextLogin({ sub: attackerSub, email: existing.email, email_verified: verified });
        const attempt = await cms.loginWithOidcRaw();
        expectRejected(await cms.meRaw(asSession(attempt.session)), `既存のアカウントのメールアドレス・email_verified=${String(verified)} でのSSOログイン`);
        const identities = await listMyIdentities.call(asSession(await cms.login(existing)), anyTarget);
        expectSuccess(identities, "既存のアカウントのひも付けの一覧");
        expectNotLeaked(identities, attackerSub, "既存のアカウントのひも付けの一覧");
      }

      // 対照: email_verified が真で、ドメインが一致すればJITでアカウントが作られる
      const email = jitEmail("verified");
      idp.setNextLogin({ sub: sub(), email, email_verified: true });
      const ok = await cms.loginWithOidcRaw();
      expectSuccess(await cms.meRaw(asSession(ok.session)), "email_verified=true でのSSOログイン");
      expect(idp.tokenRequests.at(-1)?.pkceVerified, "PKCEが使われていない").toBe(true);
    });
  },
  180_000,
);
