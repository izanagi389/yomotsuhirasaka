// サーバーに同梱する変換器のコピーが、ビルドしたものと同じか（JSで書く: 型の検査で Node の型を要らなくするため）。
// 違っていたら `npm run sync:server -w @new-cms/converter` を実行する。
import { readFileSync } from "node:fs";
import { expect, test } from "vitest";

test("the server's copy of the converter is up to date", () => {
  const built = readFileSync(new URL("../dist/index.js", import.meta.url), "utf8");
  const copy = readFileSync(new URL("../../../server/src/script/converter.js", import.meta.url), "utf8");
  expect(copy, "run `npm run sync:server -w @new-cms/converter`").toBe(built);
});
