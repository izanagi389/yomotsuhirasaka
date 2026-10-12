// 約束のテストを登録する関数。すべての約束のテストは `inv()` で登録する。
//
// - テストの名前は `<テスト名>: <約束の文>` になる。
// - 約束の文は invariants.md と一字一句同じでなければならない（メタテストで確かめる）。
// - マイルストーンが current-milestone より先のもの、または pending.txt に載っているものは実行を保留する。

import { test } from "vitest";
import { loadCurrentMilestone, loadInvariants, loadPending, milestoneReached } from "./invariants-doc.js";

const rows = new Map(loadInvariants().map((r) => [r.testName, r]));
const current = loadCurrentMilestone();
const pending = new Set(loadPending());

/**
 * @param testName invariants.md の「テスト名」列の値（例: `inv_auth_01`）
 * @param statement invariants.md の「約束」列の文をそのまま書く
 * @param fn テストの本体
 */
export function inv(testName: string, statement: string, fn: () => Promise<void>, timeoutMs?: number): void {
  const row = rows.get(testName);
  if (!row) {
    throw new Error(`${testName} は docs/invariants.md にありません`);
  }
  if (row.statement !== statement) {
    throw new Error(`${testName} の約束の文が docs/invariants.md と一致しません`);
  }
  const title = `${testName}: ${statement}`;
  if (!milestoneReached(row.milestone, current)) {
    test.skip(`${title}（${row.milestone}のため保留）`, fn);
  } else if (pending.has(testName)) {
    test.skip(`${title}（未着手: pending.txt）`, fn);
  } else {
    test(title, fn, timeoutMs);
  }
}
