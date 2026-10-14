// `cms import`: Markdown のフォルダを取り込む（03-content-and-editing.md「既存ブログからの取り込み」、Q74, Q132）。
//
// 1. すべてのファイルを手元で検査する（frontmatter、本文の変換、独自部品の対応、画像のファイル）。
//    1つでも問題があれば、何も送らずに止める（INV-IMP-01）。
// 2. 画像をアップロードする（ファイル名は送らない。保存先の名前はサーバーが決める。INV-MED-04）。
// 3. 記事をまとめて取り込みのAPIに送る（サーバーが1つのトランザクションで作成・公開する）。
//    失敗したら、アップロードした画像を消す。
// 4. リダイレクトの設定（Nuxt の routeRules）を書き出す（INV-IMP-03）。

import { readdirSync, readFileSync, realpathSync, renameSync, statSync, writeFileSync } from "node:fs";
import { dirname, extname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { parse as parseYaml } from "yaml";
import { ApiError, Client } from "../api.js";
import type { ImportConfig } from "./config.js";
import { prepare } from "./components.js";
import { convert, type BodyRules, type ImageRef, type TiptapNode } from "./markdown.js";

export interface ImportOptions {
  sourceDir: string;
  workspaceId: string;
  config: ImportConfig;
  redirectOutput: string;
  /** サイトのルートからのパス（`/images/a.png`）の画像を探すディレクトリ。 */
  staticDir?: string;
}

export interface Logger {
  info(msg: string): void;
  warn(msg: string): void;
  error(msg: string): void;
}

/** 終了コード: 0 成功、1 取り込めない内容（何も取り込んでいない）、3 通信やサーバーの失敗。 */
export type ExitCode = 0 | 1 | 3;

interface Field {
  name: string;
  type: "text" | "slug" | "richtext" | "reference";
}
interface ContentType {
  name: string;
  urlTemplate: string;
  fields: Field[];
}
interface Manifest {
  locales: string[];
  defaultLocale: string;
  body: { nodes: { name: string }[]; marks: { name: string }[] };
  contentTypes: ContentType[];
}

interface Item {
  key: string;
  locale: string;
  fields: Record<string, unknown>;
  originalUrl: string;
  translationKey?: string;
  translationOf?: string;
}

const MEDIA_TYPES: Record<string, string> = {
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".gif": "image/gif",
  ".webp": "image/webp",
};

/** フォルダの中の .md と .mdx（隠しフォルダと node_modules は見ない）。 */
function markdownFiles(root: string): string[] {
  const out: string[] = [];
  const walk = (dir: string) => {
    for (const name of readdirSync(dir).sort()) {
      if (name.startsWith(".") || name === "node_modules") continue;
      const path = join(dir, name);
      const st = statSync(path);
      if (st.isDirectory()) walk(path);
      else if (st.isFile() && [".md", ".mdx"].includes(extname(name).toLowerCase())) out.push(path);
    }
  };
  walk(root);
  return out;
}

/** frontmatter と本文に分ける。本文が何行目から始まるかも返す。 */
function splitFrontmatter(text: string): { data: string; body: string; bodyLine: number } | null {
  const normalized = text.replace(/^﻿/, "").replace(/\r\n?/g, "\n");
  if (!normalized.startsWith("---\n")) return null;
  const end = normalized.indexOf("\n---", 3);
  if (end < 0) return null;
  const after = normalized.indexOf("\n", end + 4);
  const data = normalized.slice(4, end);
  const body = after < 0 ? "" : normalized.slice(after + 1);
  return { data, body, bodyLine: data.split("\n").length + 3 };
}

/** `child` が `root` の中にあるか（実体のパスで比べる。シンボリックリンクで外に出られないように）。 */
function inside(root: string, child: string): boolean {
  const rel = relative(root, child);
  return rel !== "" && !rel.startsWith("..") && !isAbsolute(rel) && !rel.split(sep).includes("..");
}

/** 画像のパスを、ファイルの実体のパスにする。外に出るパスや、ないファイルは問題にする。 */
function resolveImage(ref: ImageRef, label: string, file: string, sourceRoot: string, staticRoot: string | null): { path: string } | { problem: string } {
  const where = `${label}:${ref.line}`;
  if (/^[a-z][a-z0-9+.-]*:/i.test(ref.src)) return { problem: `${where}: remote images are not supported; download them into the folder first (${ref.src})` };
  let decoded: string;
  try {
    decoded = decodeURIComponent(ref.src.split(/[?#]/)[0]!);
  } catch {
    return { problem: `${where}: the image path cannot be read (${ref.src})` };
  }
  if (decoded.includes("\0") || decoded.includes("\\")) return { problem: `${where}: the image path is not allowed (${ref.src})` };
  let base: string;
  let candidate: string;
  if (decoded.startsWith("/")) {
    if (!staticRoot) return { problem: `${where}: the image path starts with "/"; pass --static-dir to say where those files are (${ref.src})` };
    base = staticRoot;
    candidate = resolve(staticRoot, "." + decoded);
  } else {
    base = sourceRoot;
    candidate = resolve(dirname(file), decoded);
  }
  let real: string;
  try {
    real = realpathSync(candidate);
  } catch {
    return { problem: `${where}: the image file does not exist (${ref.src})` };
  }
  if (!inside(base, real)) return { problem: `${where}: the image is outside the folder (${ref.src})` };
  if (!statSync(real).isFile()) return { problem: `${where}: the image is not a file (${ref.src})` };
  if (!MEDIA_TYPES[extname(real).toLowerCase()]) return { problem: `${where}: only PNG, JPEG, GIF and WebP images can be imported (${ref.src})` };
  return { path: real };
}

/** 新しいURLのパス（リダイレクトの設定に使う）。 */
const pathOf = (url: string): string => {
  const u = new URL(url, "https://placeholder.invalid");
  return u.pathname;
};

export async function runImport(client: Client, options: ImportOptions, log: Logger): Promise<ExitCode> {
  const { config } = options;
  let sourceRoot: string;
  try {
    sourceRoot = realpathSync(options.sourceDir);
  } catch {
    log.error(`the folder does not exist: ${options.sourceDir}`);
    return 1;
  }
  const staticRoot = options.staticDir ? realpathSync(options.staticDir) : null;

  // --- スキーマ ---
  let manifest: Manifest;
  try {
    const info = (await client.request("GET", `/api/admin/workspaces/${encodeURIComponent(options.workspaceId)}/schema`)) as {
      state?: string;
      manifest?: Manifest | null;
    };
    if (!info.manifest) {
      log.error("no template has been deployed to this workspace yet");
      return 1;
    }
    if (info.state !== "ok") {
      log.error("this workspace is waiting for a schema migration; try again after it completes");
      return 1;
    }
    manifest = info.manifest;
  } catch (e) {
    log.error(`cannot read the workspace's schema: ${(e as Error).message}`);
    return 3;
  }
  const ct = config.contentType
    ? manifest.contentTypes.find((c) => c.name === config.contentType)
    : manifest.contentTypes.length === 1
      ? manifest.contentTypes[0]
      : undefined;
  if (!ct) {
    log.error(config.contentType ? `unknown content type: ${config.contentType}` : 'the template has several content types; set "contentType" in the config');
    return 1;
  }
  const richtexts = ct.fields.filter((f) => f.type === "richtext");
  const bodyField = config.bodyField ?? (richtexts.length === 1 ? richtexts[0]!.name : undefined);
  if (!bodyField || !richtexts.some((f) => f.name === bodyField)) {
    log.error('cannot tell which field holds the body; set "bodyField" in the config');
    return 1;
  }
  const rules: BodyRules = { nodes: new Set(manifest.body.nodes.map((n) => n.name)), marks: new Set(manifest.body.marks.map((m) => m.name)) };
  const special = new Set([config.originalUrlKey, config.localeKey, config.translationKey, config.translationOf].filter((k): k is string => !!k));
  const ignored = new Set(config.ignoreKeys);

  // --- 1. 手元の検査 ---
  const problems: string[] = [];
  const items: Item[] = [];
  const images: { ref: ImageRef; path: string }[] = [];
  const files = markdownFiles(sourceRoot);
  if (files.length === 0) {
    log.error("no Markdown files (.md, .mdx) were found");
    return 1;
  }
  for (const path of files) {
    const file = relative(sourceRoot, path);
    const split = splitFrontmatter(readFileSync(path, "utf8"));
    if (!split) {
      problems.push(`${file}: the file must start with a frontmatter block (---)`);
      continue;
    }
    let data: unknown;
    try {
      data = parseYaml(split.data);
    } catch (e) {
      problems.push(`${file}: the frontmatter is not valid YAML: ${(e as Error).message.split("\n")[0]}`);
      continue;
    }
    if (typeof data !== "object" || data === null || Array.isArray(data)) {
      problems.push(`${file}: the frontmatter must be a mapping of keys to values`);
      continue;
    }
    const fm = data as Record<string, unknown>;
    const str = (key: string | undefined): string | undefined => {
      if (!key || fm[key] === undefined || fm[key] === null) return undefined;
      if (typeof fm[key] !== "string") {
        problems.push(`${file}: "${key}" must be a string`);
        return undefined;
      }
      return fm[key] as string;
    };
    const originalUrl = str(config.originalUrlKey);
    if (!originalUrl) problems.push(`${file}: "${config.originalUrlKey}" (the original URL) is missing`);
    const locale = str(config.localeKey) ?? manifest.defaultLocale;
    if (!manifest.locales.includes(locale)) problems.push(`${file}: locale "${locale}" is not a locale of this workspace`);

    const fields: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(fm)) {
      if (special.has(key) || ignored.has(key)) continue;
      const field = ct.fields.find((f) => f.name === key);
      if (!field) {
        problems.push(`${file}: frontmatter key "${key}" is not a field of "${ct.name}" (add it to "ignoreKeys" to skip it)`);
        continue;
      }
      if (field.type === "richtext" || field.type === "reference") {
        problems.push(`${file}: "${key}" is a ${field.type} field and cannot be set from the frontmatter`);
        continue;
      }
      if (typeof value !== "string") {
        problems.push(`${file}: "${key}" must be a string`);
        continue;
      }
      fields[key] = value;
    }

    const prepared = prepare(file, split.body, split.bodyLine, config.componentMap);
    problems.push(...prepared.problems);
    for (const w of prepared.warnings) log.warn(w);
    const converted = convert(file, prepared.text, prepared.lineOf, prepared.components, config.componentMap, rules);
    problems.push(...converted.problems);
    for (const w of converted.warnings) log.warn(w);
    for (const ref of converted.images) {
      const r = resolveImage(ref, file, path, sourceRoot, staticRoot);
      if ("problem" in r) problems.push(r.problem);
      else images.push({ ref, path: r.path });
    }
    if ((converted.doc.content ?? []).length > 0) fields[bodyField] = converted.doc;

    const item: Item = { key: file, locale, fields, originalUrl: originalUrl ?? "" };
    const tk = str(config.translationKey);
    const tof = str(config.translationOf);
    if (tk) item.translationKey = tk;
    if (tof) item.translationOf = tof;
    items.push(item);
  }
  if (problems.length > 0) {
    for (const p of problems) log.error(p);
    log.error(`nothing was imported (${problems.length} problem${problems.length === 1 ? "" : "s"})`);
    return 1;
  }

  // --- 2. 画像のアップロード（同じファイルは1回だけ） ---
  const uploaded = new Map<string, string>();
  const removeUploaded = async () => {
    for (const id of uploaded.values()) {
      try {
        await client.request("DELETE", `/api/admin/workspaces/${encodeURIComponent(options.workspaceId)}/media/${id}?force=true`);
      } catch (e) {
        log.warn(`could not remove uploaded media ${id}: ${(e as Error).message}`);
      }
    }
  };
  try {
    for (const { path } of images) {
      if (uploaded.has(path)) continue;
      const res = (await client.request("POST", `/api/admin/workspaces/${encodeURIComponent(options.workspaceId)}/media`, {
        bytes: readFileSync(path),
        contentType: MEDIA_TYPES[extname(path).toLowerCase()]!,
      })) as { id?: unknown };
      if (typeof res?.id !== "string") throw new Error("unexpected response to the upload");
      uploaded.set(path, res.id);
    }
  } catch (e) {
    log.error(`cannot upload ${relative(sourceRoot, images.find((i) => !uploaded.has(i.path))?.path ?? "")}: ${(e as Error).message}`);
    await removeUploaded();
    log.error("nothing was imported");
    return e instanceof ApiError && e.status < 500 ? 1 : 3;
  }
  for (const { ref, path } of images) {
    (ref.node as TiptapNode).attrs = { ...ref.node.attrs, mediaId: uploaded.get(path)! };
  }

  // --- 3. 取り込み ---
  let result: { entries: { key: string; id: string; url: string }[] };
  try {
    result = (await client.request("POST", `/api/admin/workspaces/${encodeURIComponent(options.workspaceId)}/imports`, {
      json: { contentType: ct.name, entries: items },
    })) as typeof result;
  } catch (e) {
    log.error(`the CMS refused the import: ${(e as Error).message}`);
    await removeUploaded();
    log.error("nothing was imported");
    return e instanceof ApiError && e.status < 500 ? 1 : 3;
  }

  // --- 4. リダイレクトの設定（Nuxt の routeRules） ---
  const byKey = new Map(items.map((i) => [i.key, i]));
  const routeRules: Record<string, { redirect: { to: string; statusCode: 301 } }> = {};
  for (const e of result.entries) {
    const from = pathOf(byKey.get(e.key)!.originalUrl);
    const to = pathOf(e.url);
    if (from !== to) routeRules[from] = { redirect: { to, statusCode: 301 } };
  }
  const tmp = `${options.redirectOutput}.tmp-${process.pid}`;
  writeFileSync(tmp, JSON.stringify({ routeRules }, null, 2) + "\n");
  renameSync(tmp, options.redirectOutput);

  log.info(`imported ${result.entries.length} entries and ${uploaded.size} images`);
  log.info(`wrote ${Object.keys(routeRules).length} redirects to ${options.redirectOutput} (Nuxt routeRules)`);
  return 0;
}
