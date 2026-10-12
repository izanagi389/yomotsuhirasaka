// 認証の約束: INV-AUTH-01, 02, 03, 07, 08

import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asSession, driver, type Auth, type Session } from "./support/driver.js";
import { expectNotLeaked, expectRejected, expectSuccess } from "./support/assert.js";
import { parseSetCookie } from "./support/http.js";
import { operationsOf } from "./support/operations.js";
import { disposableTarget, lazyWorld } from "./support/world.js";

const world = lazyWorld();

/** パスワードのハッシュとして使われる代表的な書式。 */
const PASSWORD_HASH_PATTERN = /\$argon2(?:id|i|d)?\$|\$2[abxy]\$\d\d\$|\$scrypt\$|\$pbkdf2/;

inv("inv_auth_01", "ログインしていない利用者は、管理APIを使えない", async () => {
  const w = await world();
  const target = await disposableTarget(w, w.a);

  // 対照: ログインしていれば使える
  expectSuccess(await w.cms.meRaw(asSession(w.a.adminSession)), "ログインした利用者の本人情報の取得");

  for (const op of operationsOf(w.cms)) {
    const res = await op.call({ kind: "none" }, target);
    expectRejected(res, `ログインせずに ${op.name}`);
    expectNotLeaked(res, w.a.secretMarker, `ログインせずに ${op.name}`);
  }
});

inv("inv_auth_02", "ログアウトしたセッションや、有効期限の切れたセッションは使えない", async () => {
  const w = await world();

  // ログアウトしたセッション
  const session = await w.cms.login(w.a.editor);
  expectSuccess(await w.cms.meRaw(asSession(session)), "ログアウト前の本人情報の取得");
  await w.cms.logout(session);
  expectRejected(await w.cms.meRaw(asSession(session)), "ログアウトしたセッションでの本人情報の取得");

  // 有効期限の切れたセッション: 有効期限を短くしたサーバーを別に起動して確かめる
  const config = { ...(await driver.validServerConfig()), sessionLifetimeSeconds: 2 };
  const server = await driver.startServer(config);
  try {
    expect(server.outcome, `有効期限を短くしたサーバーが起動しない:\n${server.output}`).toBe("ready");
    const cms = server.client!;
    const short = await cms.login(w.a.editor);
    expectSuccess(await cms.meRaw(asSession(short)), "有効期限内の本人情報の取得");
    await new Promise((r) => setTimeout(r, 4_000));
    expectRejected(await cms.meRaw(asSession(short)), "有効期限の切れたセッションでの本人情報の取得");
  } finally {
    await server.stop();
  }
}, 180_000);

inv("inv_auth_03", "管理APIのどの応答にも、パスワードやそのハッシュは含まれない", async () => {
  const w = await world();
  const target = await disposableTarget(w, w.a);
  const passwords = [w.serverAdmin.password, w.a.admin.password, w.a.editor.password];

  const check = (res: { text: string; headers: Headers }, what: string) => {
    const all = res.text + "\n" + [...res.headers.entries()].map(([k, v]) => `${k}: ${v}`).join("\n");
    for (const p of passwords) {
      expect(all.includes(p), `${what} の応答にパスワードが含まれている`).toBe(false);
    }
    expect(PASSWORD_HASH_PATTERN.test(all), `${what} の応答にパスワードのハッシュらしき値が含まれている`).toBe(false);
  };

  // ログインの応答（成功と失敗）
  check(await w.cms.loginRaw(w.a.editor), "ログイン");
  check(await w.cms.loginRaw({ ...w.a.editor, password: w.a.admin.password }), "誤ったパスワードでのログイン");

  // 読み取りの操作すべてを、サーバー管理者とワークスペースの管理者で呼ぶ
  // （状態を変える操作は、パスワードの変更などで舞台を壊すため、ここでは呼ばない）
  const sessions: [string, Session][] = [
    ["サーバー管理者", w.serverAdminSession],
    ["ワークスペースの管理者", w.a.adminSession],
  ];
  for (const [who, session] of sessions) {
    for (const op of operationsOf(w.cms).filter((o) => !o.mutating)) {
      check(await op.call(asSession(session), target), `${who}の ${op.name}`);
    }
  }
});

inv("inv_auth_07", "CSRFトークンのない、状態を変えるリクエストは拒否される", async () => {
  const w = await world();
  const target = await disposableTarget(w, w.a);
  await w.cms.reauthenticate(w.a.adminSession, w.a.admin);
  // 別のセッションのCSRFトークン（同じ利用者の別のログイン）
  const other = await w.cms.login(w.a.admin);
  const ops = operationsOf(w.cms);

  for (const op of ops.filter((o) => o.mutating)) {
    const missing: Auth = { kind: "session", session: w.a.adminSession, csrf: "missing" };
    expectRejected(await op.call(missing, target), `CSRFトークンなしの ${op.name}`);
    const wrong: Auth = { kind: "session", session: w.a.adminSession, csrf: { token: other.csrfToken } };
    expectRejected(await op.call(wrong, target), `別のセッションのCSRFトークンでの ${op.name}`);
  }

  // 対照: 同じ操作も、正しいCSRFトークンがあれば通る
  const createEntry = ops.find((o) => o.name === "createEntry")!;
  expectSuccess(await createEntry.call(asSession(w.a.adminSession), target), "正しいCSRFトークンでの createEntry");

  // 拒否されたリクエストでセッションが壊れていない（拒否の理由がCSRFだけである）
  expectSuccess(await w.cms.meRaw(asSession(w.a.adminSession)), "拒否の後の本人情報の取得");
});

inv(
  "inv_auth_08",
  "管理画面のセッションのCookieには、HttpOnly・Secure・SameSite=Strict が付き、名前に `__Host-` の接頭辞が付く",
  async () => {
    const w = await world();
    const session = await w.cms.login(w.a.editor);

    // セッションのCookie = 取り除くとログインしていない扱いになるCookie
    const sessionCookieNames: string[] = [];
    for (const name of session.cookies.keys()) {
      const cookies = new Map(session.cookies);
      cookies.delete(name);
      const res = await w.cms.meRaw(asSession({ ...session, cookies }));
      if (res.status === 401 || res.status === 403) sessionCookieNames.push(name);
    }
    expect(sessionCookieNames.length, "ログインの状態を保つCookieが見つからない").toBeGreaterThan(0);

    for (const name of sessionCookieNames) {
      const headers = session.setCookieHeaders.map(parseSetCookie).filter((c) => c.name === name);
      expect(headers.length, `${name} を設定した Set-Cookie が見つからない`).toBeGreaterThan(0);
      const c = headers[headers.length - 1]!;
      expect(c.name.startsWith("__Host-"), `${name} に __Host- の接頭辞がない`).toBe(true);
      expect(c.attributes.has("httponly"), `${name} に HttpOnly がない`).toBe(true);
      expect(c.attributes.has("secure"), `${name} に Secure がない`).toBe(true);
      expect(c.attributes.get("samesite")?.toLowerCase(), `${name} の SameSite`).toBe("strict");
      // __Host- の条件（ブラウザが受け付けるための前提）
      expect(c.attributes.get("path"), `${name} の Path`).toBe("/");
      expect(c.attributes.has("domain"), `${name} に Domain が付いている`).toBe(false);
    }
  },
);
