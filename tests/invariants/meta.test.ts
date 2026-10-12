// 約束の一覧・テスト・未着手の一覧（pending.txt）が食い違っていないことを確かめる。
// CMSを起動しなくても動く。CIで常に実行する。

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, test } from "vitest";
import { loadCurrentMilestone, loadInvariants, loadPending, milestoneReached, testsDir } from "./support/invariants-doc.js";

const invariants = loadInvariants();
const current = loadCurrentMilestone();
const pending = loadPending();
const byName = new Map(invariants.map((r) => [r.testName, r]));

function registeredTests(): { file: string; testName: string; statement: string }[] {
  const out: { file: string; testName: string; statement: string }[] = [];
  const call = /\binv\(\s*"(inv_[a-z]+_\d+)",\s*"([^"]*)"/g;
  for (const file of readdirSync(testsDir).filter((f) => f.endsWith(".test.ts") && f !== "meta.test.ts")) {
    const text = readFileSync(join(testsDir, file), "utf8");
    for (const m of text.matchAll(call)) {
      out.push({ file, testName: m[1]!, statement: m[2]! });
    }
  }
  return out;
}

const registered = registeredTests();

describe("約束の一覧とテストの整合", () => {
  test("invariants.md のIDとテスト名が対応している（INV-ABC-01 → inv_abc_01）", () => {
    for (const r of invariants) {
      expect(r.testName, r.id).toBe(r.id.toLowerCase().replace(/^inv-/, "inv_").replace(/-/g, "_"));
    }
    expect(new Set(invariants.map((r) => r.id)).size).toBe(invariants.length);
  });

  test("テストが参照する約束は invariants.md にあり、約束の文が一字一句同じ", () => {
    for (const t of registered) {
      const row = byName.get(t.testName);
      expect(row, `${t.file}: ${t.testName} が invariants.md にない`).toBeDefined();
      expect(t.statement, `${t.file}: ${t.testName} の約束の文`).toBe(row!.statement);
    }
  });

  test("pending.txt のテスト名は invariants.md にあり、重複していない", () => {
    for (const name of pending) {
      expect(byName.has(name), `pending.txt の ${name} が invariants.md にない`).toBe(true);
    }
    expect(new Set(pending).size, "pending.txt に重複がある").toBe(pending.length);
  });

  test(`現在のマイルストーン（${current}）以前の約束には、テストがあるか、pending.txt に載っている`, () => {
    const tested = new Set(registered.map((t) => t.testName));
    const pendingSet = new Set(pending);
    const missing = invariants
      .filter((r) => milestoneReached(r.milestone, current))
      .filter((r) => !tested.has(r.testName) && !pendingSet.has(r.testName))
      .map((r) => r.testName);
    expect(missing, "テストも pending.txt の記載もない約束").toEqual([]);
  });
});
