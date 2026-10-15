#!/usr/bin/env node
// CMSのCLI。M1では、テンプレートのデプロイ（`cms deploy`・`cms schema build`）と、既存のブログからの取り込み（`cms import`）。
// メッセージは英語（CLAUDE.md「言語」）。トークンは環境変数 CMS_TOKEN で渡す（引数に書くとプロセスの一覧に出るため）。

import { writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { ApiError, Client, checkBaseUrl } from "./api.js";
import { ConfigError, loadConfig } from "./import/config.js";
import { runImport, type Logger } from "./import/run.js";
import { deploy } from "./schema/deploy.js";
import { TemplateError, loadTemplate } from "./schema/template.js";

const USAGE = `Usage:
  cms deploy <template folder> --url <CMS URL> [--dry-run] [--allow-destructive] [--allow-broken-queries]
  cms schema build <template folder> [--out <manifest .json>]
  cms import <folder> --url <CMS URL> --workspace <workspace ID> --redirects <output.json>
             [--config <import config .json>] [--static-dir <folder>]

The template folder is templates/<template name>/ and holds template.json.

Environment:
  CMS_TOKEN  personal token: "deploy" scope for deploy (server administrator), "write" scope for import`;

const log: Logger = {
  info: (m) => process.stdout.write(`${m}\n`),
  warn: (m) => process.stderr.write(`warning: ${m}\n`),
  error: (m) => process.stderr.write(`error: ${m}\n`),
};

function token(): string | null {
  const t = process.env["CMS_TOKEN"];
  if (!t) log.error("set CMS_TOKEN to a personal token");
  return t || null;
}

/** `cms schema build`: マニフェストを書き出す（デプロイしない。中身を確かめるため）。 */
function schemaBuild(rest: string[]): number {
  const { values, positionals } = parseArgs({ args: rest, allowPositionals: true, options: { out: { type: "string" } } });
  if (positionals.length !== 1) {
    process.stderr.write(`${USAGE}\n`);
    return 2;
  }
  const { manifest } = loadTemplate(positionals[0]!);
  const json = `${JSON.stringify(manifest, null, 2)}\n`;
  if (values.out) writeFileSync(values.out, json);
  else process.stdout.write(json);
  return 0;
}

/** `cms deploy`: サーバーで試行してから、デプロイする。 */
async function deployCommand(rest: string[]): Promise<number> {
  const { values, positionals } = parseArgs({
    args: rest,
    allowPositionals: true,
    options: {
      url: { type: "string" },
      "dry-run": { type: "boolean", default: false },
      "allow-destructive": { type: "boolean", default: false },
      "allow-broken-queries": { type: "boolean", default: false },
    },
  });
  if (positionals.length !== 1 || !values.url) {
    process.stderr.write(`${USAGE}\n`);
    return 2;
  }
  const t = token();
  if (!t) return 2;
  const base = checkBaseUrl(values.url);
  const { name, manifest } = loadTemplate(positionals[0]!);
  return deploy(
    new Client(base, t),
    name,
    manifest,
    { dryRun: values["dry-run"], allowDestructive: values["allow-destructive"], allowBrokenQueries: values["allow-broken-queries"] },
    log,
  );
}

async function main(argv: string[]): Promise<number> {
  const [command, ...rest] = argv;
  try {
    if (command === "deploy") return await deployCommand(rest);
    if (command === "schema" && rest[0] === "build") return schemaBuild(rest.slice(1));
  } catch (e) {
    if (e instanceof TemplateError) {
      log.error(e.message);
      return 1;
    }
    if (e instanceof ApiError) {
      log.error(`${e.code}: ${e.message}`);
      return 1;
    }
    if (e instanceof TypeError && "code" in e && String(e.code).startsWith("ERR_PARSE_ARGS")) {
      process.stderr.write(`error: ${e.message}\n${USAGE}\n`);
      return 2;
    }
    if (e instanceof Error && e.message.startsWith("--url")) {
      log.error(e.message);
      return 2;
    }
    throw e;
  }
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
  const t = token();
  if (!t) return 2;
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
    new Client(base, t),
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
