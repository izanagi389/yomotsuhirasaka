import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["**/*.test.ts"],
    exclude: ["node_modules/**"],
    // 1つのCMSを共有して動かすため、ファイルごとの並列実行はしない。
    // ワークスペースは各ファイルが独自に作るので、順序には依存しない。
    fileParallelism: false,
    testTimeout: 60_000,
    hookTimeout: 120_000,
  },
});
