#!/usr/bin/env node
// npm のロックファイルの取得元が、公式のレジストリだけであることを確かめる（13-security.md「依存ライブラリ」）。
// Rust の依存の取得元は cargo-deny（deny.toml）が確かめる。
import { readFileSync } from "node:fs";

const ALLOWED = "https://registry.npmjs.org/";
let bad = 0;
for (const file of process.argv.slice(2)) {
  const lock = JSON.parse(readFileSync(file, "utf8"));
  for (const [path, entry] of Object.entries(lock.packages ?? {})) {
    if (entry.link || path === "" || !entry.resolved) continue;
    if (!entry.resolved.startsWith(ALLOWED)) {
      console.error(`${file}: ${path} comes from ${entry.resolved}`);
      bad += 1;
    }
  }
}
if (bad > 0) {
  console.error(`${bad} package(s) do not come from ${ALLOWED}`);
  process.exit(1);
}
