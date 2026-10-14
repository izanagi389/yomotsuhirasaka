// ドライバーの一部: サーバーの設定ファイルの書き出しと、プロセスの起動・停止。
// 設定ファイルの書式は server/src/config.rs に合わせる。

import { spawn } from "node:child_process";
import { chmodSync, copyFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { CliResult, CmsClient, ServerConfig, StartedServer } from "./driver.js";

function requiredEnv(name: string): string {
  const v = process.env[name];
  if (!v) throw new Error(`環境変数 ${name} が設定されていません（tests/invariants/README.md の「テスト環境」）`);
  return v;
}

/** サーバーの設定の秘密の値の名前（server/src/config.rs の SECRET_SETTING_NAMES と同じ）。 */
export const SECRET_SETTING_NAMES = ["database.password", "migration_database.password"] as const;

/** テスト環境のCMSと同じDB・鍵を使う、本番の設定として正しい設定。 */
export function validServerConfigFromEnv(): ServerConfig {
  return {
    production: true,
    devMode: false,
    secrets: {
      "database.password": requiredEnv("CMS_TEST_SERVER_DB_PASSWORD"),
      "migration_database.password": requiredEnv("CMS_TEST_SERVER_MIGRATION_DB_PASSWORD"),
    },
    encryptionKeyFile: requiredEnv("CMS_TEST_SERVER_KEY_FILE"),
    databaseUrl: requiredEnv("CMS_TEST_SERVER_DB_URL"),
    migrationDatabaseUrl: requiredEnv("CMS_TEST_SERVER_MIGRATION_DB_URL"),
    publicUrl: requiredEnv("CMS_TEST_PUBLIC_URL"),
    internalDestinationExceptions: [],
    // 舞台に多要素認証のない所属者（編集者など）を作るため、全員への必須を無効にする（Q197）
    mfaRequiredForEveryone: false,
  };
}

/** URLにパスワードが入っていれば取り出す（設定ファイルでは、パスワードは別の項目に書く）。 */
function splitPassword(url: string, fallback: string): { url: string; password: string } {
  const u = new URL(url);
  if (!u.password) return { url, password: fallback };
  const password = decodeURIComponent(u.password);
  u.password = "";
  return { url: u.toString(), password };
}

const tomlString = (s: string): string => JSON.stringify(s); // TOMLの基本文字列は、JSONの文字列と同じエスケープで書ける

/** まだサーバーが対応していない設定。指定されたら、黙って無視せずに失敗させる。 */
function unsupported(config: ServerConfig): string[] {
  const out: string[] = [];
  if (config.internalDestinationExceptions.length > 0) out.push("internalDestinationExceptions");
  for (const key of ["egressProxy", "breachedPasswordApi"] as const) {
    if (config[key] !== undefined && config[key] !== null) out.push(key);
  }
  return out;
}

export function renderConfig(config: ServerConfig, listen: string): string {
  const missing = unsupported(config);
  if (missing.length > 0) throw new Error(`ドライバーが未実装です: サーバーの設定 ${missing.join(", ")}（tests/invariants/support/server-process.ts）`);
  const db = splitPassword(config.databaseUrl, config.secrets["database.password"] ?? "");
  const migration = splitPassword(config.migrationDatabaseUrl, config.secrets["migration_database.password"] ?? "");
  const lines = [
    `production = ${config.production}`,
    `dev_mode = ${config.devMode}`,
    `public_url = ${tomlString(config.publicUrl)}`,
    `listen = ${tomlString(listen)}`,
    `log_level = ${tomlString(config.logLevel ?? "info")}`,
    // テストの実行環境（同じホスト）を信頼するプロキシにし、接続元のIPアドレスを X-Forwarded-For で渡せるようにする（Q106）
    `trusted_proxies = ["127.0.0.1", "::1"]`,
  ];
  if (config.encryptionKeyFile !== null) lines.push(`encryption_key_file = ${tomlString(config.encryptionKeyFile)}`);
  lines.push("", "[database]", `url = ${tomlString(db.url)}`, `password = ${tomlString(db.password)}`);
  lines.push("", "[migration_database]", `url = ${tomlString(migration.url)}`, `password = ${tomlString(migration.password)}`);
  if (config.loginAttemptsPerIpPerMinute !== undefined) lines.push("", "[limits]", `login_attempts_per_ip_per_minute = ${config.loginAttemptsPerIpPerMinute}`);
  // メールの送信: テスト環境の準備が起動したSMTPの受け手（ループバックなので暗号化なし）
  const smtpPort = process.env["CMS_TEST_SMTP_PORT"];
  if (smtpPort) lines.push("", "[smtp]", `host = "127.0.0.1"`, `port = ${Number(smtpPort)}`, `security = "none"`, `from = "CMS <cms@inv.test>"`);
  // メディアの保存先: テストが調べるファイルシステムの範囲（CMS_TEST_FS_ROOT）の中に置く
  lines.push("", "[media]", `root = ${tomlString(join(process.env["CMS_TEST_FS_ROOT"] ?? tmpdir(), "media"))}`);
  const session: string[] = [];
  if (config.sessionLifetimeSeconds !== undefined) session.push(`absolute_lifetime_seconds = ${config.sessionLifetimeSeconds}`);
  if (config.reauthWindowSeconds !== undefined) session.push(`reauth_window_seconds = ${config.reauthWindowSeconds}`);
  if (config.oneTimeTokenLifetimeSeconds !== undefined) session.push(`one_time_token_lifetime_seconds = ${config.oneTimeTokenLifetimeSeconds}`);
  if (session.length > 0) lines.push("", "[session]", ...session);
  // undefined ならサーバーの既定（有効）に任せる
  if (config.mfaRequiredForEveryone !== undefined) lines.push("", "[auth]", `mfa_required_for_everyone = ${config.mfaRequiredForEveryone}`);
  // 配信API: キーごとの上限は、用途（ビルド用・サーバー配信用）の両方に同じ値を使う
  const delivery: string[] = [];
  if (config.rateLimits?.deliveryPerKey !== undefined) {
    delivery.push(`build_key_requests_per_minute = ${config.rateLimits.deliveryPerKey}`, `server_key_requests_per_minute = ${config.rateLimits.deliveryPerKey}`);
  }
  if (config.rateLimits?.deliveryPerWorkspace !== undefined) delivery.push(`workspace_requests_per_minute = ${config.rateLimits.deliveryPerWorkspace}`);
  if (config.sharedCache !== undefined) delivery.push(`shared_cache = ${config.sharedCache}`);
  if (delivery.length > 0) lines.push("", "[delivery]", ...delivery);
  if (config.loginHistoryRetention) {
    const r = config.loginHistoryRetention;
    lines.push("", "[login_history]", `retention_seconds = ${r.retentionSeconds}`, `sweep_interval_seconds = ${r.sweepIntervalSeconds}`);
  }
  lines.push("");
  return lines.join("\n");
}

async function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const s = createServer();
    s.once("error", reject);
    s.listen(0, "127.0.0.1", () => {
      const port = (s.address() as { port: number }).port;
      s.close(() => resolve(port));
    });
  });
}

export async function startServerProcess(config: ServerConfig, connect: (baseUrl: string) => CmsClient, timeoutMs = 60_000): Promise<StartedServer> {
  const bin = requiredEnv("CMS_TEST_SERVER_BIN");
  const dir = mkdtempSync(join(tmpdir(), "cms-server-"));
  const port = await freePort();
  const listen = `127.0.0.1:${port}`;
  const configPath = join(dir, "config.toml");
  writeFileSync(configPath, renderConfig(config, listen), { mode: 0o600 });

  const child = spawn(bin, ["serve", "--config", configPath], { stdio: ["ignore", "pipe", "pipe"] });
  let output = "";
  child.stdout.on("data", (c: Buffer) => (output += c.toString("utf8")));
  child.stderr.on("data", (c: Buffer) => (output += c.toString("utf8")));
  let exitCode: number | null = null;
  let exited = false;
  const exitPromise = new Promise<void>((resolve) => {
    child.on("exit", (code) => {
      exitCode = code;
      exited = true;
      resolve();
    });
  });

  const baseUrl = `http://${listen}`;
  const deadline = Date.now() + timeoutMs;
  let outcome: "ready" | "exited" = "exited";
  while (Date.now() < deadline) {
    if (exited) break;
    try {
      const res = await fetch(`${baseUrl}/healthz`);
      if (res.ok) {
        outcome = "ready";
        break;
      }
    } catch {
      // まだ受け付けていない
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  if (!exited && outcome !== "ready") {
    child.kill("SIGKILL");
    await exitPromise;
    rmSync(dir, { recursive: true, force: true });
    throw new Error(`サーバーが ${timeoutMs}ms 以内に受け付けを始めず、終了もしなかった:\n${output}`);
  }
  if (outcome === "exited") await exitPromise;

  let client: CmsClient | null = null;
  return {
    outcome,
    get exitCode() {
      return exitCode;
    },
    get output() {
      return output;
    },
    get client() {
      if (outcome !== "ready") return null;
      client ??= connect(baseUrl);
      return client;
    },
    async stop() {
      if (!exited) {
        child.kill("SIGTERM");
        const timer = setTimeout(() => child.kill("SIGKILL"), 10_000);
        await exitPromise;
        clearTimeout(timer);
      }
      rmSync(dir, { recursive: true, force: true });
    },
  };
}

/** サーバーのバイナリのサブコマンド（緊急停止などのCLI）を、設定ファイルを書き出して実行する。 */
export async function runServerCli(config: ServerConfig, args: string[], timeoutMs = 120_000): Promise<CliResult> {
  const bin = requiredEnv("CMS_TEST_SERVER_BIN");
  const dir = mkdtempSync(join(tmpdir(), "cms-cli-"));
  const configPath = join(dir, "config.toml");
  // CLIは待ち受けないが、設定ファイルには必要
  writeFileSync(configPath, renderConfig(config, "127.0.0.1:0"), { mode: 0o600 });
  try {
    const child = spawn(bin, [...args, "--config", configPath], { stdio: ["ignore", "pipe", "pipe"] });
    let output = "";
    child.stdout.on("data", (c: Buffer) => (output += c.toString("utf8")));
    child.stderr.on("data", (c: Buffer) => (output += c.toString("utf8")));
    const exitCode = await new Promise<number>((resolve, reject) => {
      const timer = setTimeout(() => {
        child.kill("SIGKILL");
        reject(new Error(`CLI（${args.join(" ")}）が ${timeoutMs}ms 以内に終わらなかった:\n${output}`));
      }, timeoutMs);
      child.on("error", reject);
      child.on("exit", (code) => {
        clearTimeout(timer);
        resolve(code ?? -1);
      });
    });
    return { exitCode, output };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

/** 鍵のファイルにある鍵のバージョン（`v<版>:<Base64>` の行。書式は server/src/keys.rs）。 */
function keyVersions(path: string): number[] {
  return readFileSync(path, "utf8")
    .split("\n")
    .map((l) => /^\s*v(\d+):/.exec(l)?.[1])
    .filter((v): v is string => v !== undefined)
    .map(Number);
}

/**
 * 暗号化の鍵の入れ替え: 鍵のファイルを別の場所に写し、サーバーのCLIで
 * 新しい鍵を加える → 保存済みの秘密を暗号化し直す → 元からあった鍵を外す、を順に行う。
 * 写した鍵のファイルを使う設定を返す（元の鍵のファイルは変えない）。
 */
export async function rotateEncryptionKeyWithCli(config: ServerConfig): Promise<{ config: ServerConfig; result: CliResult }> {
  if (config.encryptionKeyFile === null) throw new Error("暗号化の鍵のファイルが設定されていません");
  const dir = mkdtempSync(join(tmpdir(), "cms-key-"));
  const keyFile = join(dir, "encryption.key");
  copyFileSync(config.encryptionKeyFile, keyFile);
  chmodSync(keyFile, 0o600);
  const next: ServerConfig = { ...config, encryptionKeyFile: keyFile };
  const steps = [
    ["add-encryption-key"],
    ["reencrypt-secrets", "--yes"],
    ...keyVersions(config.encryptionKeyFile).map((v) => ["remove-encryption-key", "--version", String(v), "--yes"]),
  ];
  let output = "";
  for (const step of steps) {
    const r = await runServerCli(next, step);
    output += `$ cms-server ${step.join(" ")}\n${r.output}\n`;
    if (r.exitCode !== 0) return { config: next, result: { exitCode: r.exitCode, output } };
  }
  return { config: next, result: { exitCode: 0, output } };
}
