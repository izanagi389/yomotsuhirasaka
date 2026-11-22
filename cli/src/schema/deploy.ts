// テンプレートのデプロイ（Q52, Q53, Q54）。
// 1. サーバーで試行する（dryRun）。判定はサーバーのもので、CLIは結果を見せるだけ
// 2. マニフェストの問題があれば止める。破壊的な変更・壊れるクエリは、明示的な指定がなければ止める
// 3. デプロイする（サーバーは同じ検査をもう一度行い、すべてのワークスペースに適用するか、どれにも適用しない）

import { ApiError, type Client } from "../api.js";
import type { Logger } from "../import/run.js";
import type { Manifest } from "./template.js";

export interface DeployOptions {
  dryRun: boolean;
  allowDestructive: boolean;
  allowBrokenQueries: boolean;
}

interface Check {
  dryRun: true;
  problems: string[];
  destructive: string[];
  brokenQueries: { workspaceId: string; queryId: string; problems: string[] }[];
  unchanged: boolean;
  /** 最後にデプロイした日時（まだなければ null）。 */
  deployedAt: string | null;
  workspaces: number;
}

interface Outcome {
  template: string;
  deployedAt: string;
  unchanged: boolean;
  /** 適用したワークスペース。 */
  workspaces: string[];
}

/** 終了コード: 0 成功、1 デプロイしなかった（問題あり）、3 想定外の失敗。 */
export async function deploy(client: Client, name: string, manifest: Manifest, options: DeployOptions, log: Logger): Promise<number> {
  const path = `/api/admin/templates/${name}/deploy`;
  const check = (await client.request("POST", path, {
    json: { manifest, dryRun: true, allowDestructive: options.allowDestructive, allowBrokenQueries: options.allowBrokenQueries },
  })) as Check;

  log.info(`template "${name}": ${check.deployedAt === null ? "not deployed yet" : `last deployed at ${check.deployedAt}`}, used by ${check.workspaces} workspace(s)`);
  if (check.problems.length > 0) {
    log.error("the template is not valid:");
    for (const p of check.problems) log.error(`  ${p}`);
    return 1;
  }
  if (check.unchanged) log.info("no changes since the last deployment");
  for (const d of check.destructive) log.warn(`destructive change: ${d}`);
  for (const q of check.brokenQueries) log.warn(`breaks registered query ${q.queryId} in workspace ${q.workspaceId}: ${q.problems.join(", ")}`);

  let blocked = false;
  if (check.destructive.length > 0 && !options.allowDestructive) {
    log.error("destructive changes: add the new field, migrate the data and remove the old one in separate deployments, or pass --allow-destructive");
    blocked = true;
  }
  if (check.brokenQueries.length > 0 && !options.allowBrokenQueries) {
    // 壊れるクエリが1つでもあれば、どのワークスペースにも適用しない（INV-SCH-03）
    log.error("registered queries would break, so nothing is deployed to any workspace: update the queries, or pass --allow-broken-queries to apply the change to every workspace anyway");
    blocked = true;
  }
  if (blocked) return 1;
  if (options.dryRun) {
    log.info("dry run: nothing was deployed");
    return 0;
  }

  let outcome: Outcome;
  try {
    outcome = (await client.request("POST", path, {
      json: { manifest, allowDestructive: options.allowDestructive, allowBrokenQueries: options.allowBrokenQueries },
    })) as Outcome;
  } catch (e) {
    // 試行の後に状況が変わった（別のデプロイなど）。サーバーの判断をそのまま伝える
    if (e instanceof ApiError && e.status < 500) {
      log.error(`${e.code}: ${e.message}`);
      return 1;
    }
    throw e;
  }
  log.info(outcome.unchanged ? `unchanged (last deployed at ${outcome.deployedAt})` : `deployed at ${outcome.deployedAt}`);
  for (const id of outcome.workspaces) log.info(`  workspace ${id}: migrated`);
  return 0;
}
