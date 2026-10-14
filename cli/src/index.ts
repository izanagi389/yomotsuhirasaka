#!/usr/bin/env node
// CMSのCLI。M1では既存のブログからの取り込み（`cms import`）だけ。
// メッセージは英語（CLAUDE.md「言語」）。トークンは環境変数 CMS_TOKEN で渡す（引数に書くとプロセスの一覧に出るため）。

import { parseArgs } from "node:util";
import { Client, checkBaseUrl } from "./api.js";
import { ConfigError, loadConfig } from "./import/config.js";
import { runImport, type Logger } from "./import/run.js";

const USAGE = `Usage:
  cms import <folder> --url <CMS URL> --workspace <workspace ID> --redirects <output.json>
             [--config <import config .json>] [--static-dir <folder>]

Environment:
  CMS_TOKEN  personal token with the "write" scope for the workspace (required)`;

const log: Logger = {
  info: (m) => process.stdout.write(`${m}\n`),
  warn: (m) => process.stderr.write(`warning: ${m}\n`),
  error: (m) => process.stderr.write(`error: ${m}\n`),
};

async function main(argv: string[]): Promise<number> {
  const [command, ...rest] = argv;
  if (command !== "import") {
    process.stderr.write(`${USAGE}\n`);
    return 2;
  }
  let parsed;
  try {
    parsed = parseArgs({
      args: rest,
      allowPositionals: true,
      options: {
        url: { type: "string" },
        workspace: { type: "string" },
        config: { type: "string" },
        redirects: { type: "string" },
        "static-dir": { type: "string" },
      },
    });
  } catch (e) {
    process.stderr.write(`error: ${(e as Error).message}\n${USAGE}\n`);
    return 2;
  }
  const { values, positionals } = parsed;
  if (positionals.length !== 1 || !values.url || !values.workspace || !values.redirects) {
    process.stderr.write(`${USAGE}\n`);
    return 2;
  }
  const token = process.env["CMS_TOKEN"];
  if (!token) {
    log.error("set CMS_TOKEN to a personal token");
    return 2;
  }
  let base: URL;
  let config;
  try {
    base = checkBaseUrl(values.url);
    config = loadConfig(values.config);
  } catch (e) {
    log.error((e as Error).message);
    return e instanceof ConfigError ? 1 : 2;
  }
  return runImport(
    new Client(base, token),
    { sourceDir: positionals[0]!, workspaceId: values.workspace, config, redirectOutput: values.redirects, staticDir: values["static-dir"] },
    log,
  );
}

main(process.argv.slice(2)).then(
  (code) => process.exit(code),
  (e: unknown) => {
    log.error(e instanceof Error ? e.message : String(e));
    process.exit(3);
  },
);
