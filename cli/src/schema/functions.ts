// テンプレートの関数（URL生成関数・フック・カスタムノードの変換関数）を、TSのファイルからJSのソースにする（Q49）。
// - template.json には `{ "file": "./functions/url.ts", ... }` と書く。CLIが esbuild で1つのESモジュールにまとめ、
//   マニフェストの `{ "source": "...", ... }` にする。サーバーはJSのソースだけを受け取る（バイトコードは使わない）
// - ファイルと、その中の import はテンプレートのフォルダの中の相対パスだけを許す（npm のパッケージや node: は使わない）。
//   `import type` は変換で消えるので、型の入口（@yomotsuhirasaka/cli/types）は使える
// - フォルダの外を指すパス（`..`、絶対パス、外に出るシンボリックリンク）は止める

import { realpathSync, statSync } from "node:fs";
import { isAbsolute, relative, resolve, sep } from "node:path";
import { build, type Plugin } from "esbuild";
import { TemplateError } from "./errors.js";

/** 1つの関数のソースの大きさの上限（サーバーと同じ）。 */
export const MAX_SOURCE_BYTES = 512 * 1024;

/** realpath がフォルダの中にあるか。 */
function inside(root: string, path: string): boolean {
  const rel = relative(root, path);
  return rel !== "" && !rel.startsWith(`..${sep}`) && rel !== ".." && !isAbsolute(rel);
}

/** テンプレートのフォルダの中の、実在するファイルの realpath。 */
function resolveInside(root: string, from: string, spec: string, what: string): string {
  if (isAbsolute(spec)) throw new TemplateError(`${what}: use a path relative to the template folder (got an absolute path)`);
  const path = resolve(from, spec);
  let real: string;
  try {
    real = realpathSync(path);
  } catch {
    throw new TemplateError(`${what}: cannot find ${spec}`);
  }
  if (!inside(root, real)) throw new TemplateError(`${what}: ${spec} is outside the template folder`);
  if (!statSync(real).isFile()) throw new TemplateError(`${what}: ${spec} is not a file`);
  return real;
}

/** import をテンプレートのフォルダの中の相対パスに限る esbuild のプラグイン。 */
function onlyInside(root: string, what: string): Plugin {
  return {
    name: "yomi-only-inside-template",
    setup(b) {
      b.onResolve({ filter: /.*/ }, (args) => {
        if (args.kind === "entry-point") return { path: args.path };
        if (!args.path.startsWith("./") && !args.path.startsWith("../")) {
          return {
            errors: [{ text: `${what}: only relative imports inside the template folder are allowed (got "${args.path}"; use "import type" for types)` }],
          };
        }
        const candidates = [args.path, `${args.path}.ts`, `${args.path}.js`, `${args.path}/index.ts`];
        for (const c of candidates) {
          try {
            return { path: resolveInside(root, args.resolveDir, c, what) };
          } catch (e) {
            if (e instanceof TemplateError && / is outside the template folder|an absolute path/.test(e.message)) return { errors: [{ text: e.message }] };
          }
        }
        return { errors: [{ text: `${what}: cannot find "${args.path}"` }] };
      });
    },
  };
}

/** 1つの関数のファイルを、1つのESモジュールのJSにする。 */
export async function bundleFunction(templateDir: string, file: string, what: string): Promise<string> {
  const root = realpathSync(templateDir);
  const entry = resolveInside(root, root, file, what);
  let result;
  try {
    result = await build({
      entryPoints: [entry],
      bundle: true,
      write: false,
      format: "esm",
      platform: "neutral",
      target: "es2023",
      minify: false,
      sourcemap: false,
      legalComments: "none",
      logLevel: "silent",
      plugins: [onlyInside(root, what)],
    });
  } catch (e) {
    const errors = (e as { errors?: { text: string }[] }).errors ?? [];
    throw new TemplateError(errors.length > 0 ? errors.map((x) => x.text).join("; ") : `${what}: ${(e as Error).message}`);
  }
  const source = result.outputFiles[0]?.text ?? "";
  if (Buffer.byteLength(source) > MAX_SOURCE_BYTES) {
    throw new TemplateError(`${what}: the bundled function is larger than ${MAX_SOURCE_BYTES} bytes`);
  }
  return source;
}

type Json = Record<string, unknown>;
const isObject = (v: unknown): v is Json => typeof v === "object" && v !== null && !Array.isArray(v);

/** template.json の関数（`{file, permissions, allowedHosts}`）をマニフェストの形（`{source, ...}`）にする。 */
async function toScript(templateDir: string, value: unknown, what: string): Promise<unknown> {
  if (value === undefined || value === null) return value;
  if (!isObject(value)) throw new TemplateError(`${what}: must be an object with "file"`);
  if ("source" in value) throw new TemplateError(`${what}: write the function in a TypeScript file and refer to it with "file"`);
  const { file, ...rest } = value;
  if (typeof file !== "string" || file === "") throw new TemplateError(`${what}.file: must be a path to a TypeScript file`);
  return { source: await bundleFunction(templateDir, file, `${what}.file`), ...rest };
}

const HOOK_KINDS = ["beforeSave", "beforePublish", "afterSave", "afterPublish", "afterUnpublish"];

/** テンプレートの中のすべての関数をJSのソースにした、新しいオブジェクトを返す（元のオブジェクトは変えない）。 */
export async function bundleFunctions(templateDir: string, template: Json): Promise<Json> {
  const out: Json = structuredClone(template);
  const body = out["body"];
  if (isObject(body) && Array.isArray(body["customNodes"])) {
    for (const [i, node] of (body["customNodes"] as unknown[]).entries()) {
      if (!isObject(node)) continue;
      for (const key of ["toMarkdown", "toHtml"]) {
        if (key in node) node[key] = await toScript(templateDir, node[key], `body.customNodes[${i}].${key}`);
      }
    }
  }
  if (Array.isArray(out["contentTypes"])) {
    for (const [i, ct] of (out["contentTypes"] as unknown[]).entries()) {
      if (!isObject(ct)) continue;
      if ("urlFunction" in ct) ct["urlFunction"] = await toScript(templateDir, ct["urlFunction"], `contentTypes[${i}].urlFunction`);
      const hooks = ct["hooks"];
      if (isObject(hooks)) {
        for (const kind of HOOK_KINDS) {
          if (kind in hooks) hooks[kind] = await toScript(templateDir, hooks[kind], `contentTypes[${i}].hooks.${kind}`);
        }
      }
    }
  }
  return out;
}
