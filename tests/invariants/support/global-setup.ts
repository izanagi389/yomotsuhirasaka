// テストの舞台のCMS（CMS_TEST_URL）が渡されていなければ、テスト環境の設定（scripts/test-env/setup.sh）で
// CMSを起動し、サーバー管理者（多要素認証を設定済み）を作り、BASE_TEMPLATE をデプロイしてから、テストに渡す。
//
// - CMS_TEST_URL が設定されていれば、何もしない（外で用意した環境を使う）。
// - CMS_TEST_SERVER_BIN がなければ何もしない（舞台を使うテストは、環境変数がないことを理由に失敗する）。
// - CMSはテストの実行環境と同じホストで、HTTPで待ち受ける。公開URLは https のまま（Cookieなどの扱いは本番と同じ）。
// - メールは、ここで起動するSMTPの受け手（support/smtp-sink.ts）で受ける。
// - 擬似LLM（support/mock-llm.ts）も、ここで別のプロセスとして起動する（CMS_TEST_MOCK_LLM_URL が設定されていなければ）。

import { randomUUID } from "node:crypto";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { asBearer, driver, type StartedServer } from "./driver.js";
import { isSuccess } from "./http.js";
import { startMockLlmProcess, type StartedMockLlm } from "./mock-llm.js";
import { runServerCli, validServerConfigFromEnv } from "./server-process.js";
import { startSmtpSink, type SmtpSink } from "./smtp-sink.js";
import { BASE_TEMPLATE } from "./template.js";

let server: StartedServer | null = null;
let smtp: SmtpSink | null = null;
let mailDir: string | null = null;
let mockLlm: StartedMockLlm | null = null;

export async function setup(): Promise<void> {
  if (process.env["CMS_TEST_URL"] || !process.env["CMS_TEST_SERVER_BIN"]) return;
  if (!process.env["CMS_TEST_MOCK_LLM_URL"]) {
    mockLlm = await startMockLlmProcess();
    process.env["CMS_TEST_MOCK_LLM_URL"] = mockLlm.baseUrl;
  }
  try {
    await setupCms();
  } catch (e) {
    // 途中で失敗したら、起動したもの（擬似LLMなど）を止めてから失敗する
    await teardown();
    throw e;
  }
}

async function setupCms(): Promise<void> {
  // メールの受け口（招待やリセットのメールを受け取る）
  mailDir = mkdtempSync(join(tmpdir(), "cms-inv-mail-"));
  smtp = await startSmtpSink(mailDir);
  process.env["CMS_TEST_MAIL_DIR"] = mailDir;
  process.env["CMS_TEST_SMTP_PORT"] = String(smtp.port);
  // 舞台の準備のログインは同じ接続元に集まるので、接続元ごとのログインの上限を大きくする
  // （回数制限を確かめるテストは、接続元を名乗るか、自分でサーバーを起動する）
  const config = { ...validServerConfigFromEnv(), loginAttemptsPerIpPerMinute: 100_000 };
  server = await driver.startServer(config);
  if (server.outcome !== "ready" || !server.client) throw new Error(`テストの舞台のCMSが起動しない:\n${server.output}`);
  const cms = server.client;

  // サーバー管理者（パスワードはCLIが作って一度だけ表示する）
  const email = `inv-root-${randomUUID().slice(0, 8)}@inv.test`;
  const created = await runServerCli(config, ["create-server-admin", "--email", email]);
  const password = created.output.match(/Password \(shown only once\): (\S+)/)?.[1];
  if (created.exitCode !== 0 || !password) throw new Error(`サーバー管理者を作れない:\n${created.output}`);
  const session = await cms.loginPasswordOnly({ email, password });
  const totpSecret = await cms.enrollTotp(session, { email, password });
  const creds = { email, password, totpSecret };

  // 擬似LLMに届くこと（CLIの llm-check。舞台の準備の確認）
  if (config.llm) {
    const llmCheck = await runServerCli(config, ["llm-check"]);
    if (llmCheck.exitCode !== 0) throw new Error(`テストの舞台のCMSから擬似LLMに届かない:\n${llmCheck.output}`);
  }

  // BASE_TEMPLATE のデプロイ
  const admin = await cms.login(creds);
  const token = await cms.deployToken(admin, creds);
  const res = await cms.deployTemplateRaw(asBearer(token), BASE_TEMPLATE);
  if (!isSuccess(res)) throw new Error(`BASE_TEMPLATE をデプロイできない（${res.status}）: ${res.text}`);

  process.env["CMS_TEST_URL"] = cms.baseUrl;
  process.env["CMS_TEST_SERVER_ADMIN_EMAIL"] = email;
  process.env["CMS_TEST_SERVER_ADMIN_PASSWORD"] = password;
  process.env["CMS_TEST_SERVER_ADMIN_TOTP_SECRET"] = totpSecret;
}

export async function teardown(): Promise<void> {
  await server?.stop();
  server = null;
  await smtp?.close();
  smtp = null;
  if (mailDir) rmSync(mailDir, { recursive: true, force: true });
  mailDir = null;
  await mockLlm?.stop();
  mockLlm = null;
}
