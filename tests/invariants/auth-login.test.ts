// ログインの約束: INV-AUTH-04, 05, 17, 21, 22, 24, 25

import { expect } from "vitest";
import { randomBytes } from "node:crypto";
import { inv } from "./support/inv.js";
import { asSession, driver } from "./support/driver.js";
import { expectRejected, expectRefused, expectSuccess } from "./support/assert.js";
import { expectIndistinguishable } from "./support/compare.js";
import { env } from "./support/env.js";
import { isSuccess } from "./support/http.js";
import { sha1Hex, startMockBreachApi } from "./support/hibp.js";
import { mailCursor, waitForToken } from "./support/mail.js";
import { startOidcIdp } from "./support/oidc-idp.js";
import { disposableTarget, lazyWorld, uniqueEmail, uniqueIp } from "./support/world.js";
import { REQUIRED_OPERATIONS, operationsOf } from "./support/operations.js";

const world = lazyWorld();

/** 十分に強い、一意なパスワード。 */
const strongPassword = (): string => `inv-${randomBytes(18).toString("base64url")}`;

/** アカウント単位の制限を確かめるため、失敗ごとに接続元を変えて誤ったパスワードを送る。 */
async function failLogins(times: number, email: string): Promise<void> {
  const w = await world();
  for (let i = 0; i < times; i++) {
    await w.cms.loginRaw({ email, password: strongPassword() }, { extras: { clientIp: uniqueIp() } });
  }
}

inv("inv_auth_04", "ログインの失敗が続いた後は、一定時間、正しいパスワードでもログインできない", async () => {
  const w = await world();
  const user = await w.cms.createMember(w.a.adminSession, w.a.id, "editor");

  // 対照: 失敗が続く前は、正しいパスワードで入れる
  expectSuccess(await w.cms.loginRaw(user, { extras: { clientIp: uniqueIp() } }), "失敗が続く前のログイン");

  await failLogins(10, user.email);

  // 失敗に使っていない接続元からでも、正しいパスワードで入れない（アカウント単位の制限）
  const res = await w.cms.loginRaw(user, { extras: { clientIp: uniqueIp() } });
  expect(isSuccess(res), "失敗が続いた直後に、正しいパスワードでログインできてしまった").toBe(false);
  await expect(w.cms.login(user, { extras: { clientIp: uniqueIp() } }), "失敗が続いた直後のログイン").rejects.toThrow();
});

inv(
  "inv_auth_17",
  "ログインの待ち時間中の応答は、待ち時間でないときにパスワードを誤った場合の応答と区別できない",
  async () => {
    const w = await world();
    const waiting = await w.cms.createMember(w.a.adminSession, w.a.id, "editor");
    const normal = await w.cms.createMember(w.a.adminSession, w.a.id, "editor");

    await failLogins(10, waiting.email);

    // 待ち時間中に正しいパスワードを送った応答 と、待ち時間でない人がパスワードを誤った応答
    const duringWait = await w.cms.loginRaw(waiting, { extras: { clientIp: uniqueIp() } });
    const wrongPassword = await w.cms.loginRaw({ ...normal, password: strongPassword() }, { extras: { clientIp: uniqueIp() } });
    expect(isSuccess(duringWait), "待ち時間中にログインできてしまった").toBe(false);
    expectIndistinguishable(duringWait, wrongPassword, "待ち時間中の応答と、パスワードを誤った応答");

    // 待ち時間中に誤ったパスワードを送った場合も同じ
    const duringWaitWrong = await w.cms.loginRaw({ ...waiting, password: strongPassword() }, { extras: { clientIp: uniqueIp() } });
    expectIndistinguishable(duringWaitWrong, wrongPassword, "待ち時間中の誤ったパスワードの応答と、パスワードを誤った応答");
  },
);

inv(
  "inv_auth_05",
  "多要素認証を設定していないサーバー管理者とワークスペースの管理者は、管理操作を実行できない。SSOでログインした場合も、CMS側のパスキーを設定していなければ実行できない",
  async () => {
    const w = await world();
    const { cms } = w;

    // ワークスペースの管理者（多要素認証なし）
    const wsAdmin = await cms.createMember(w.a.adminSession, w.a.id, "admin", { mfa: false });
    let session = await cms.login(wsAdmin);
    await cms.reauthenticate(session, wsAdmin);
    expectRejected(await cms.inviteRaw(session, w.a.id, uniqueEmail("ten05x"), "editor"), "多要素認証のないワークスペースの管理者による招待");
    const target = await disposableTarget(w, w.a);
    const changeRole = cms.adminOperations.find((o) => o.name === "changeMemberRole")!;
    expectRejected(await changeRole.call(asSession(session), target), "多要素認証のないワークスペースの管理者によるロールの変更");
    // 対照: 多要素認証を設定すれば実行できる
    const totpSecret = await cms.enrollTotp(session, wsAdmin);
    const withMfa = { ...wsAdmin, totpSecret };
    session = await cms.login(withMfa);
    await cms.reauthenticate(session, withMfa);
    expectSuccess(await cms.inviteRaw(session, w.a.id, uniqueEmail("auth05"), "editor"), "多要素認証を設定したワークスペースの管理者による招待");

    // サーバー管理者（多要素認証なし）
    const serverAdmin = await cms.createServerAdmin({ mfa: false });
    let saSession = await cms.login(serverAdmin);
    await cms.reauthenticate(saSession, serverAdmin);
    expectRejected(await cms.createWorkspaceRaw(asSession(saSession), "inv-auth-05-nomfa"), "多要素認証のないサーバー管理者によるワークスペースの作成");
    const saTotp = await cms.enrollTotp(saSession, serverAdmin);
    const saWithMfa = { ...serverAdmin, totpSecret: saTotp };
    saSession = await cms.login(saWithMfa);
    await cms.reauthenticate(saSession, saWithMfa);
    expectSuccess(await cms.createWorkspaceRaw(asSession(saSession), "inv-auth-05-mfa"), "多要素認証を設定したサーバー管理者によるワークスペースの作成");

    // SSOでログインした管理者（TOTPは設定済みだが、CMS側のパスキーはない）
    const idp = await startOidcIdp({ ...env.oidc, issuer: env.oidc.issuer });
    try {
      const ssoAdmin = await cms.createMember(w.a.adminSession, w.a.id, "admin");
      const pwSession = await cms.login(ssoAdmin);
      const sub = `sub-${randomBytes(8).toString("hex")}`;
      idp.setNextLogin({ sub, email: ssoAdmin.email, email_verified: true });
      await cms.linkOidcIdentity(pwSession);

      idp.setNextLogin({ sub, email: ssoAdmin.email, email_verified: true });
      const sso = await cms.loginWithOidcRaw();
      expectSuccess(await cms.meRaw(asSession(sso.session)), "SSOでログインした管理者の本人情報の取得");
      expectRejected(await cms.inviteRaw(sso.session, w.a.id, uniqueEmail("auth05sso"), "editor"), "パスキーのないSSOの管理者による招待");

      // 対照: パスキーを設定し、パスキーで再認証すれば実行できる
      const passkey = await cms.enrollPasskey(sso.session, ssoAdmin);
      await cms.reauthenticateWithPasskey(sso.session, passkey);
      expectSuccess(await cms.inviteRaw(sso.session, w.a.id, uniqueEmail("auth05sso2"), "editor"), "パスキーを設定したSSOの管理者による招待");
    } finally {
      await idp.close();
    }
  },
  180_000,
);

/** 単純なパターンのパスワード（どれも12文字以上）。`local` はメールアドレスの @ より前。 */
const simplePatterns = (local: string): string[] => [
  "aaaaaaaaaaaa", // 同じ文字の繰り返し
  "123456789012", // 連続した数字
  "abcdefghijkl", // 連続した文字
  "qwertyuiop12", // キーボードの並び
  "1qaz2wsx3edc", // キーボードの並び（縦）
  `${local}#2026`, // メールアドレスを含む
];

inv(
  "inv_auth_21",
  "12文字未満のパスワードと、単純なパターン（同じ文字の繰り返し、連続した文字や数字、キーボードの並び、メールアドレスを含むもの）のパスワードは、登録・変更のときに拒否される",
  async () => {
    const w = await world();
    const { cms } = w;
    const tooShort = "Abc!23456789"; // 12文字
    const elevenChars = tooShort.slice(0, 11);

    // 変更のとき
    const user = await cms.createMember(w.a.adminSession, w.a.id, "editor");
    const session = await cms.login(user);
    await cms.reauthenticate(session, user);
    expectRefused(await cms.changePasswordRaw(session, user.password, elevenChars), "11文字のパスワードへの変更");
    for (const p of simplePatterns(user.email.split("@")[0]!)) {
      expect(p.length).toBeGreaterThanOrEqual(12);
      expectRefused(await cms.changePasswordRaw(session, user.password, p), `単純なパターンのパスワード（${p}）への変更`);
    }
    // 対照: 強いパスワードには変更できる
    expectSuccess(await cms.changePasswordRaw(session, user.password, strongPassword()), "強いパスワードへの変更");

    // 登録のとき（招待の承諾でパスワードを決める）
    const email = uniqueEmail("auth21");
    const seen = await mailCursor(email);
    expectSuccess(await cms.inviteRaw(w.a.adminSession, w.a.id, email, "editor"), "招待");
    const token = await waitForToken(email, seen, "invite");
    expectRefused(await cms.acceptInviteRaw(token, { newPassword: elevenChars }), "11文字のパスワードでの登録");
    for (const p of simplePatterns(email.split("@")[0]!)) {
      expectRefused(await cms.acceptInviteRaw(token, { newPassword: p }), `単純なパターンのパスワード（${p}）での登録`);
    }
    expectSuccess(await cms.acceptInviteRaw(token, { newPassword: strongPassword() }), "強いパスワードでの登録");
  },
);

inv(
  "inv_auth_22",
  "漏えいデータベースへの照会を有効にしても、外部に送られるのはパスワードのハッシュの先頭5文字だけで、照会先に届かないときも登録は止まらない",
  async () => {
    const w = await world();
    const api = await startMockBreachApi();
    const config = { ...(await driver.validServerConfig()), breachedPasswordApi: { baseUrl: api.baseUrl(env.harnessAllowedHost) } };
    const server = await driver.startServer(config);
    try {
      expect(server.outcome, `照会を有効にしたサーバーが起動しない:\n${server.output}`).toBe("ready");
      const cms = server.client!;
      const user = await w.cms.createMember(w.a.adminSession, w.a.id, "editor");
      const session = await cms.login(user);
      await cms.reauthenticate(session, user);

      // 照会されたパスワードについて、送られたのは SHA-1 の先頭5文字だけ
      const fresh = strongPassword();
      expectSuccess(await cms.changePasswordRaw(session, user.password, fresh), "漏えいしていないパスワードへの変更");
      const hash = sha1Hex(fresh);
      const sent = api.receiver.requests;
      expect(sent.length, "照会先に問い合わせが届いていない").toBeGreaterThan(0);
      for (const r of sent) {
        expect(r.path.split("?")[0]!.toUpperCase(), "照会のパス").toBe(`/RANGE/${hash.slice(0, 5)}`);
        const everything = `${r.path}\n${JSON.stringify(r.headers)}\n${r.body}`.toUpperCase();
        expect(everything.includes(fresh.toUpperCase()), "照会にパスワードそのものが含まれている").toBe(false);
        expect(everything.includes(hash.slice(5)), "照会にハッシュの6文字目以降が含まれている").toBe(false);
      }

      // 対照: 漏えいしたパスワードは拒否される（照会が実際に使われている）
      const breached = strongPassword();
      api.addBreached(breached);
      expectRefused(await cms.changePasswordRaw(session, fresh, breached), "漏えいしたパスワードへの変更");

      // 照会先に届かないときも、変更は止まらない
      api.setDown(true);
      expectSuccess(await cms.changePasswordRaw(session, fresh, strongPassword()), "照会先が落ちているときのパスワードの変更");
    } finally {
      await server.stop();
      await api.close();
    }
  },
  180_000,
);

inv("inv_auth_24", "新しい端末の判定用のCookieを持っていても、多要素認証は省略されない", async () => {
  const w = await world();
  const user = await w.cms.createMember(w.a.adminSession, w.a.id, "admin"); // 多要素認証あり
  const first = await w.cms.login(user);
  const deviceCookies = new Map(first.cookies); // 新しい端末の判定用のCookieを含む
  await w.cms.logout(first);

  // パスワードだけで、前回のCookieを持ってログインしても、ログインした状態にならない
  const passwordOnly = await w.cms.loginPasswordOnly(user, { cookies: deviceCookies });
  expectRejected(await w.cms.meRaw(asSession(passwordOnly)), "端末のCookieとパスワードだけでの本人情報の取得");

  // 対照: 多要素認証まで済ませればログインできる
  const full = await w.cms.login(user, { cookies: deviceCookies });
  expectSuccess(await w.cms.meRaw(asSession(full)), "多要素認証まで済ませたログイン");
});

inv(
  "inv_auth_25",
  "全員への多要素認証の必須が有効なとき（既定）、多要素認証を設定していない利用者は、多要素認証の設定に必要な操作（本人の情報の確認、多要素認証の設定、再認証、ログアウト）以外の管理APIを使えない",
  async () => {
    const w = await world();
    const target = await disposableTarget(w, w.a);
    // 多要素認証のない所属者（テスト環境のCMSは、舞台を作るために全員への必須を無効にしてある）
    const user = await w.cms.createMember(w.a.adminSession, w.a.id, "editor", { mfa: false });

    // 既定の設定（全員への必須が有効）のサーバー
    const server = await driver.startServer({ ...(await driver.validServerConfig()), mfaRequiredForEveryone: undefined });
    try {
      expect(server.outcome, `既定の設定のサーバーが起動しない:\n${server.output}`).toBe("ready");
      const cms = server.client!;
      const session = await cms.login(user);

      // 本人の情報の確認はできる
      expectSuccess(await cms.meRaw(asSession(session)), "多要素認証のない利用者の本人情報の取得");
      // それ以外の管理APIは使えない
      const blocked = Object.keys(REQUIRED_OPERATIONS).filter((n) => n !== "me" && n !== "logout");
      for (const op of operationsOf(cms).filter((o) => blocked.includes(o.name))) {
        expectRejected(await op.call(asSession(session), target), `多要素認証のない利用者の ${op.name}`);
      }

      // 対照: 多要素認証を設定すれば使える（設定そのものは、多要素認証がなくてもできる）
      const totpSecret = await cms.enrollTotp(session, user);
      const withMfa = await cms.login({ ...user, totpSecret });
      for (const name of ["listWorkspaces", "getWorkspace", "listMySessions"]) {
        const op = operationsOf(cms).find((o) => o.name === name)!;
        expectSuccess(await op.call(asSession(withMfa), target), `多要素認証を設定した後の ${name}`);
      }
    } finally {
      await server.stop();
    }
  },
  180_000,
);
