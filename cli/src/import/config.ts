// 取り込みの設定ファイル（JSON）。03-content-and-editing.md「既存ブログからの取り込み」。
//
// {
//   "contentType": "post",              // 省略可（テンプレートのコンテンツタイプが1つなら）
//   "originalUrlKey": "url",            // 元のURLを持つ frontmatter のキー（必須の値）
//   "localeKey": "locale",              // ロケールのキー（なければ既定のロケール）
//   "translationKey": "translationKey", // 翻訳グループにまとめるキー（省略可）
//   "translationOf": "translationOf",   // 元の記事のスラッグを指すキー（省略可）
//   "bodyField": "body",                // 本文を入れるフィールド（省略時は本文のフィールドが1つならそれ）
//   "componentMap": { "Callout": "blockquote" }, // 独自部品 → 本文のノード
//   "ignoreKeys": ["date", "tags"]      // 取り込まない frontmatter のキー
// }

import { readFileSync } from "node:fs";

export interface ImportConfig {
  contentType?: string;
  originalUrlKey: string;
  localeKey: string;
  translationKey?: string;
  translationOf?: string;
  bodyField?: string;
  componentMap: Record<string, string>;
  ignoreKeys: string[];
}

const KNOWN = new Set(["contentType", "originalUrlKey", "localeKey", "translationKey", "translationOf", "bodyField", "componentMap", "ignoreKeys"]);

export class ConfigError extends Error {}

function optionalString(v: unknown, name: string): string | undefined {
  if (v === undefined) return undefined;
  if (typeof v !== "string" || v.length === 0) throw new ConfigError(`config: "${name}" must be a non-empty string`);
  return v;
}

export function parseConfig(raw: unknown): ImportConfig {
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) throw new ConfigError("config: must be a JSON object");
  const o = raw as Record<string, unknown>;
  const unknown = Object.keys(o).filter((k) => !KNOWN.has(k));
  if (unknown.length > 0) throw new ConfigError(`config: unknown keys: ${unknown.join(", ")}`);
  const componentMap = o["componentMap"] ?? {};
  if (typeof componentMap !== "object" || componentMap === null || Array.isArray(componentMap)) {
    throw new ConfigError('config: "componentMap" must be an object of component name to node name');
  }
  for (const [k, v] of Object.entries(componentMap)) {
    if (typeof v !== "string" || v.length === 0) throw new ConfigError(`config: componentMap.${k} must be a node name`);
  }
  const ignoreKeys = o["ignoreKeys"] ?? [];
  if (!Array.isArray(ignoreKeys) || !ignoreKeys.every((k) => typeof k === "string")) {
    throw new ConfigError('config: "ignoreKeys" must be an array of strings');
  }
  return {
    contentType: optionalString(o["contentType"], "contentType"),
    originalUrlKey: optionalString(o["originalUrlKey"], "originalUrlKey") ?? "url",
    localeKey: optionalString(o["localeKey"], "localeKey") ?? "locale",
    translationKey: optionalString(o["translationKey"], "translationKey"),
    translationOf: optionalString(o["translationOf"], "translationOf"),
    bodyField: optionalString(o["bodyField"], "bodyField"),
    componentMap: componentMap as Record<string, string>,
    ignoreKeys: ignoreKeys as string[],
  };
}

export function loadConfig(path: string | undefined): ImportConfig {
  if (!path) return parseConfig({});
  let raw: unknown;
  try {
    raw = JSON.parse(readFileSync(path, "utf8"));
  } catch (e) {
    throw new ConfigError(`config: cannot read ${path}: ${(e as Error).message}`);
  }
  return parseConfig(raw);
}
