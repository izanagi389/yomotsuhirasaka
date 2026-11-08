// 起動時の検査の約束: INV-KEY-02, INV-CFG-01
//
// 設定を1か所だけ変えてサーバーを起動し、受け付けを始めずに終了することを確かめる。
// 正しい設定なら起動することも、対照として確かめる。

import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { driver, type ServerConfig } from "./support/driver.js";
import { env } from "./support/env.js";

const STARTUP_TIMEOUT = 300_000;

async function expectStarts(config: ServerConfig, what: string): Promise<void> {
  const server = await driver.startServer(config);
  try {
    expect(server.outcome, `${what}: 起動するべきなのに終了した:\n${server.output}`).toBe("ready");
  } finally {
    await server.stop();
  }
}

async function expectRefusesToStart(config: ServerConfig, what: string): Promise<void> {
  const server = await driver.startServer(config);
  try {
    expect(server.outcome, `${what}: 起動を止めるべきなのに、受け付けを始めた`).toBe("exited");
    expect(server.exitCode, `${what}: 終了コードが0`).not.toBe(0);
    expect(server.output.trim().length, `${what}: 起動を止めた理由が表示されていない`).toBeGreaterThan(0);
  } finally {
    await server.stop();
  }
}

inv("inv_key_02", "暗号化の鍵が設定されていなければ、サーバーは起動しない", async () => {
  const valid = await driver.validServerConfig();
  await expectStarts(valid, "正しい設定");
  await expectRefusesToStart({ ...valid, encryptionKeyFile: null }, "暗号化の鍵の設定なし");
  await expectRefusesToStart({ ...valid, encryptionKeyFile: "/nonexistent/yomi-encryption.key" }, "存在しない鍵のファイル");
}, STARTUP_TIMEOUT);

inv(
  "inv_cfg_01",
  "本番の設定では、秘密の値が空・短すぎる・既知の初期値である場合、DBの接続ユーザー（普段の接続、移行・ロールの作成用の接続のどちらか）がスーパーユーザーである場合、PostgreSQLが16より古い場合、開発モードが有効な場合、HTTPS前提の設定がない場合に、サーバーは起動しない",
  async () => {
    const valid = await driver.validServerConfig();
    expect(valid.production, "validServerConfig は本番の設定であること").toBe(true);
    expect(valid.devMode, "validServerConfig は開発モードが無効であること").toBe(false);
    await expectStarts(valid, "正しい設定");

    // 秘密の値
    const names = driver.secretSettingNames();
    expect(names.length, "秘密の値の設定が1つもない").toBeGreaterThan(0);
    for (const name of names) {
      for (const [label, value] of [["空", ""], ["短すぎる", "abc123"], ["既知の初期値 changeme", "changeme"], ["既知の初期値 password", "password"]] as const) {
        await expectRefusesToStart({ ...valid, secrets: { ...valid.secrets, [name]: value } }, `${name} が${label}`);
      }
    }

    // DBの接続ユーザーがスーパーユーザー
    await expectRefusesToStart({ ...valid, databaseUrl: env.dbSuperuserUrl }, "普段の接続がスーパーユーザー");
    await expectRefusesToStart({ ...valid, migrationDatabaseUrl: env.dbSuperuserUrl }, "移行・ロールの作成用の接続がスーパーユーザー");

    // PostgreSQLが16より古い
    // （PostgreSQL 15 の側にも、普段用と移行用の正しいロールを用意し、古さ以外の理由で止まらないようにする）
    await expectRefusesToStart({ ...valid, databaseUrl: env.pg15Url, migrationDatabaseUrl: env.pg15MigrationUrl }, "PostgreSQL 15");

    // 開発モード
    await expectRefusesToStart({ ...valid, devMode: true }, "本番の設定で開発モードが有効");

    // HTTPS前提の設定がない
    await expectRefusesToStart({ ...valid, publicUrl: valid.publicUrl.replace(/^https:/, "http:") }, "公開するURLが http");
  },
  STARTUP_TIMEOUT * 4,
);
