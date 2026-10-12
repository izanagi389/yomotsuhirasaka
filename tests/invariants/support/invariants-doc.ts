// docs/invariants.md・current-milestone・pending.txt を読み取る。
// テストの対象とマイルストーンは、ここで読んだ資料の内容だけで決まる。

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
export const testsDir = resolve(here, "..");
export const repoRoot = resolve(testsDir, "..", "..");

export const MILESTONES = ["M1", "M2", "M3", "M4"] as const;
export type Milestone = (typeof MILESTONES)[number];

export interface InvariantRow {
  id: string; // 例: INV-AUTH-01
  milestone: Milestone;
  statement: string; // 「約束」列の文
  testName: string; // 例: inv_auth_01
}

const ROW = /^\| (INV-[A-Z]+-\d+) \| (\S+) \| (.*) \| (.*) \| `(inv_[a-z]+_\d+)` \|$/;

export function loadInvariants(): InvariantRow[] {
  const text = readFileSync(resolve(repoRoot, "docs/invariants.md"), "utf8");
  const rows: InvariantRow[] = [];
  for (const line of text.split("\n")) {
    const m = ROW.exec(line);
    if (!m) continue;
    const [, id, milestone, statement, , testName] = m as unknown as [string, string, string, string, string, string];
    if (!(MILESTONES as readonly string[]).includes(milestone)) {
      throw new Error(`invariants.md: ${id} のマイルストーン「${milestone}」が不明です`);
    }
    rows.push({ id, milestone: milestone as Milestone, statement, testName });
  }
  return rows;
}

export function loadCurrentMilestone(): Milestone {
  const value = readFileSync(resolve(repoRoot, "current-milestone"), "utf8").trim();
  if (!(MILESTONES as readonly string[]).includes(value)) {
    throw new Error(`current-milestone の値「${value}」が不明です`);
  }
  return value as Milestone;
}

/** pending.txt に載っているテスト名（`#` 以降と空行は無視）。 */
export function loadPending(): string[] {
  const text = readFileSync(resolve(testsDir, "pending.txt"), "utf8");
  return text
    .split("\n")
    .map((line) => line.replace(/#.*$/, "").trim())
    .filter((line) => line.length > 0);
}

export function milestoneReached(m: Milestone, current: Milestone): boolean {
  return MILESTONES.indexOf(m) <= MILESTONES.indexOf(current);
}
