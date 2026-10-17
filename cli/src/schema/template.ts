// テンプレートのファイル（templates/<テンプレート名>/template.json）を読み、デプロイのAPIが受け取るマニフェストにする（Q49, Q57）。
// - 本文のノードとマークは、管理画面と同じTiptapの拡張機能（@new-cms/editor-schema）から書き出す。テンプレートには書かない
// - 関数（URL生成関数・フック・カスタムノードの変換関数）は `{ "file": "./x.ts" }` と書き、ここでJSのソースにする（functions.ts）
// - ここでの検査は早く知らせるためのもの。正しいかどうかはサーバーが決める（13-security.md「デプロイ」）

import { readFileSync } from "node:fs";
import { basename, join, resolve } from "node:path";
import { bodyDefinition, standardExtensions } from "@new-cms/editor-schema";
import { checkTemplateFunctions } from "./checks.js";
import { TemplateError } from "./errors.js";
import { bundleFunctions } from "./functions.js";

export { TemplateError };

/** テンプレート名（サーバーと同じ条件: 英小文字・数字・ハイフン、1〜64文字）。 */
export function templateName(dir: string): string {
  const name = basename(resolve(dir));
  if (!/^[a-z0-9-]{1,64}$/.test(name)) {
    throw new TemplateError(`template folder name must be 1 to 64 characters of a-z, 0-9 and - (got "${name}")`);
  }
  return name;
}

export interface Manifest {
  locales: unknown;
  defaultLocale: unknown;
  body: { nodes: unknown[]; marks: unknown[]; embedProviders: unknown };
  contentTypes: unknown;
}

/** テンプレートのファイルの中身から、マニフェストを組み立てる。 */
export function toManifest(template: unknown): Manifest {
  if (typeof template !== "object" || template === null || Array.isArray(template)) {
    throw new TemplateError("template.json must contain a JSON object");
  }
  const { $schema: _schema, body, ...rest } = template as Record<string, unknown>;
  if (body !== undefined && (typeof body !== "object" || body === null || Array.isArray(body))) {
    throw new TemplateError("body must be an object");
  }
  const { nodes, marks, ...bodyRest } = (body ?? {}) as Record<string, unknown>;
  if (nodes !== undefined || marks !== undefined) {
    throw new TemplateError("body.nodes and body.marks are written by the CLI from the editor's extensions; remove them from template.json");
  }
  const definition = bodyDefinition(standardExtensions());
  return {
    ...(rest as Omit<Manifest, "body">),
    body: { nodes: definition.nodes, marks: definition.marks, embedProviders: bodyRest["embedProviders"] ?? [], ...bodyRest },
  };
}

/** テンプレートのフォルダを読み、関数をJSのソースにしたマニフェストを返す。 */
export async function loadTemplate(dir: string): Promise<{ name: string; manifest: Manifest }> {
  const name = templateName(dir);
  const file = join(dir, "template.json");
  let text: string;
  try {
    text = readFileSync(file, "utf8");
  } catch {
    throw new TemplateError(`cannot read ${file}`);
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch (e) {
    throw new TemplateError(`${file} is not valid JSON: ${(e as Error).message}`);
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    throw new TemplateError("template.json must contain a JSON object");
  }
  const problems = checkTemplateFunctions(parsed as Record<string, unknown>);
  if (problems.length > 0) throw new TemplateError(`the template is not valid:\n  ${problems.join("\n  ")}`);
  return { name, manifest: toManifest(await bundleFunctions(dir, parsed as Record<string, unknown>)) };
}
