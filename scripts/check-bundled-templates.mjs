#!/usr/bin/env node
// サーバーに埋め込む同梱のテンプレート（server/bundled-templates/<名前>.json）が、templates/<名前> を
// `yomi schema build` でまとめ直した結果と一致することを確かめる（13-security.md「デプロイ」の同梱のテンプレート）。
// 一致しなければ、作り直すコマンドを表示して失敗する。CLI をビルドしてから実行する（cli/dist）。
import { readdirSync, readFileSync } from "node:fs";
import { basename, join } from "node:path";

const { loadTemplate } = await import(new URL("../cli/dist/schema/template.js", import.meta.url));
const dir = new URL("../server/bundled-templates/", import.meta.url);
let bad = 0;
for (const file of readdirSync(dir).filter((f) => f.endsWith(".json"))) {
  const name = basename(file, ".json");
  const { manifest } = await loadTemplate(join("templates", name));
  const expected = `${JSON.stringify(manifest, null, 2)}\n`;
  const actual = readFileSync(new URL(file, dir), "utf8");
  if (actual !== expected) {
    console.error(`server/bundled-templates/${file} is out of date; run: node cli/dist/index.js schema build templates/${name} --out server/bundled-templates/${file}`);
    bad += 1;
  }
}
if (bad > 0) process.exit(1);
console.log("bundled templates are up to date");
