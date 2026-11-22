// 評価の道具の共通の場所と、setup.ts が書いて evaluate.ts が読む接続の情報（work/state.json）。

import { readFileSync } from "node:fs";
import { join } from "node:path";

export const EVAL_DIR = join(import.meta.dirname, "..");
export const REPO_ROOT = join(EVAL_DIR, "..", "..");
export const STATE_FILE = join(EVAL_DIR, "work", "state.json");
export const BASE_URL = `http://127.0.0.1:${process.env["YOMI_EVAL_PORT"] ?? "8090"}`;

export interface WorkspaceState {
  id: string;
  readKey: string;
  queryId: string;
  /** 記事の ID から slug。 */
  slugs: Record<string, string>;
}

export interface State {
  /** 評価用の CMS の埋め込みモデル（init.sh で決めたもの）。 */
  model: string;
  workspaces: Record<string, WorkspaceState>;
}

/** init.sh が書いた設定ファイルから、埋め込みモデルの名前を読む。 */
export function configuredModel(): string {
  const config = readFileSync(join(EVAL_DIR, "work", "yomi", "config.toml"), "utf8");
  const model = config.match(/^embedding_model = "([^"]+)"$/m)?.[1];
  if (!model) throw new Error("embedding_model not found in work/yomi/config.toml");
  return model;
}

export const loadState = (): State => JSON.parse(readFileSync(STATE_FILE, "utf8")) as State;
