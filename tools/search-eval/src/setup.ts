// 評価用の CMS（init.sh で起動したもの）に、評価のデータを入れて、埋め込みができるまで待つ。
//
//   node tools/search-eval/src/setup.ts --own <Markdown のフォルダ> --own-config <取り込みの設定>
//
// 1. サーバー管理者を作り、多要素認証を設定する（CMS の決まりどおり。管理の操作に要る）
// 2. templates/blog をデプロイし、データセットごとにワークスペースと、その管理者（招待）を作って、取り込みの CLI で入れる
// 3. 配信 API のキーを発行し、検索のクエリを登録する
// 4. 埋め込みの使用量が増えなくなるまで待つ
// 接続の情報（キーなど）は work/state.json に書く（init.sh で毎回消える。リポジトリには入れない）。

import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { parseArgs } from "node:util";
import { Cms, type Credentials } from "./cms.ts";
import { BASE_URL, EVAL_DIR, REPO_ROOT, STATE_FILE, configuredModel, type State } from "./state.ts";

const SERVER_ADMIN_EMAIL = "eval-server-admin@example.com";
const TEMPLATE = "blog";
/** 埋め込みの使用量を見る間隔と、増えなくなったと見なすまでの回数。 */
const POLL_INTERVAL_MS = 10_000;
const STABLE_POLLS = 6;

interface Source {
  name: string;
  /** 取り込む Markdown のフォルダ（1回の取り込みの上限を超える分は、複数のフォルダに分けてある）。 */
  dirs: string[];
  importConfig: string;
}

function compose(args: string[], input?: string): string {
  return execFileSync("docker", ["compose", "-f", join(EVAL_DIR, "compose.yml"), ...args], { encoding: "utf8", input });
}

function yomiCli(args: string[], token: string): void {
  execFileSync("node", [join(REPO_ROOT, "cli", "dist", "index.js"), ...args], {
    stdio: "inherit",
    env: { ...process.env, YOMI_TOKEN: token },
  });
}

/** ログインして多要素認証を設定し、設定した後の状態でログインし直した接続を返す（管理の操作に多要素認証が要るため）。 */
async function loginWithMfa(email: string, password: string): Promise<Cms> {
  const cms = new Cms(BASE_URL);
  const creds: Credentials = { email, password };
  await cms.login(creds);
  creds.totpSecret = await cms.enrollTotp();
  await cms.login(creds);
  return cms;
}

async function serverAdmin(): Promise<Cms> {
  const out = compose(["exec", "-T", "yomi", "yomi-server", "create-server-admin", "--config", "/data/config.toml", "--email", SERVER_ADMIN_EMAIL]);
  const password = out.match(/Password \(shown only once\): (\S+)/)?.[1];
  if (!password) throw new Error("create-server-admin: no password in the output");
  return loginWithMfa(SERVER_ADMIN_EMAIL, password);
}

/** ワークスペースの管理者を招待して、その接続を返す（記事の取り込みとキーの発行は、ワークスペースの所属者が行う）。 */
async function workspaceAdmin(server: Cms, workspaceId: string, name: string): Promise<Cms> {
  const email = `eval-${name}@example.com`;
  const password = randomBytes(24).toString("base64url");
  await server.acceptInvite(await server.invite(workspaceId, email, "admin"), password);
  return loginWithMfa(email, password);
}

async function waitForEmbeddings(admins: { cms: Cms; workspaceId: string }[]): Promise<void> {
  let last = -1;
  let stable = 0;
  while (stable < STABLE_POLLS) {
    await new Promise((r) => setTimeout(r, POLL_INTERVAL_MS));
    const used = (await Promise.all(admins.map((a) => a.cms.embeddingTokensUsed(a.workspaceId)))).reduce((a, b) => a + b, 0);
    stable = used > 0 && used === last ? stable + 1 : 0;
    last = used;
    console.log(`embedding tokens used: ${used}${stable > 0 ? ` (unchanged ${stable}/${STABLE_POLLS})` : ""}`);
  }
}

const subfolders = (dir: string): string[] =>
  readdirSync(dir, { withFileTypes: true })
    .filter((e) => e.isDirectory())
    .map((e) => join(dir, e.name))
    .sort();

async function main(): Promise<void> {
  const { values } = parseArgs({ options: { own: { type: "string" }, "own-config": { type: "string" } } });
  if (!values.own || !values["own-config"]) throw new Error("usage: setup.ts --own <markdown folder> --own-config <import config>");
  const sources: Source[] = [
    { name: "own", dirs: [values.own], importConfig: values["own-config"] },
    { name: "jqara", dirs: subfolders(join(EVAL_DIR, "data", "jqara", "docs")), importConfig: join(EVAL_DIR, "data", "jqara", "import.json") },
  ];

  const server = await serverAdmin();
  yomiCli(["deploy", join(REPO_ROOT, "templates", TEMPLATE), "--url", BASE_URL], await server.createPersonalToken(["deploy"], []));

  const state: State = { model: configuredModel(), workspaces: {} };
  const admins: { cms: Cms; workspaceId: string }[] = [];
  for (const source of sources) {
    const id = await server.createWorkspace(`eval-${source.name}`, TEMPLATE);
    const cms = await workspaceAdmin(server, id, source.name);
    admins.push({ cms, workspaceId: id });
    const token = await cms.createPersonalToken(["read", "write"], [id]);
    source.dirs.forEach((dir, i) => {
      const redirects = join(EVAL_DIR, "work", `redirects-${source.name}-${i + 1}.json`);
      yomiCli(["import", dir, "--url", BASE_URL, "--workspace", id, "--config", source.importConfig, "--redirects", redirects], token);
    });
    const readKey = await cms.createApiKey(id, ["read"]);
    const queryId = await cms.registerSearchQuery(id, await cms.createApiKey(id, ["register"]));
    state.workspaces[source.name] = { id, readKey, queryId, slugs: Object.fromEntries(await cms.entrySlugs(id)) };
  }
  await waitForEmbeddings(admins);
  writeFileSync(STATE_FILE, JSON.stringify(state), { mode: 0o600 });
  console.log(`ready: ${STATE_FILE}`);
}

await main();
