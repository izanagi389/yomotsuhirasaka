// ビルドした変換器を、サーバーに同梱するファイルにコピーする（サーバーのビルドに Node を要らなくするため、コピーはコミットする）。
// 最新かどうかは test/server-copy.test.ts が確かめる。
import { copyFileSync } from "node:fs";
copyFileSync(new URL("../dist/index.js", import.meta.url), new URL("../../../server/src/script/converter.js", import.meta.url));
console.log("copied dist/index.js to server/src/script/converter.js");
