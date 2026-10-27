// 擬似LLMを起動する（コンテナと、約束のテストのハーネスが使う）。
// 環境変数: PORT（既定 8080。0 なら空いているポート）、HOST（既定 0.0.0.0）。
// 起動したら、標準出力に `mock-llm listening on <URL>` を1行書く（ハーネスはこの行を待つ）。

import { startMockLlm } from "./server.js";

const port = Number(process.env["PORT"] ?? "8080");
if (!Number.isInteger(port) || port < 0 || port > 65535) {
  console.error(`invalid PORT: ${process.env["PORT"]}`);
  process.exit(2);
}
const mock = await startMockLlm({ port, host: process.env["HOST"] ?? "0.0.0.0" });
console.log(`mock-llm listening on ${mock.url}`);

const stop = () => {
  void mock.close().then(() => process.exit(0));
};
process.on("SIGTERM", stop);
process.on("SIGINT", stop);
