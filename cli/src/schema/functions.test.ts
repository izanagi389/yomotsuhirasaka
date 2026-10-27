import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { checkTemplateFunctions } from "./checks.js";
import { bundleFunction, bundleFunctions } from "./functions.js";
import { scaffoldNode } from "./scaffold.js";
import { TemplateError, loadTemplate } from "./template.js";

function folder(files: Record<string, string>): string {
  const root = mkdtempSync(join(tmpdir(), "cms-fn-"));
  const dir = join(root, "blog");
  for (const [path, text] of Object.entries(files)) {
    mkdirSync(join(dir, path, ".."), { recursive: true });
    writeFileSync(join(dir, path), text);
  }
  return dir;
}

test("functions become one ES module without imports", async () => {
  const dir = folder({
    "functions/url.ts": `import { prefix } from "./shared";\nimport type { UrlInput } from "@new-cms/cli/types";\nexport default function url(e: UrlInput): string { return prefix + e.slug; }\n`,
    "functions/shared.ts": `export const prefix: string = "https://example.com/";\n`,
  });
  const source = await bundleFunction(dir, "./functions/url.ts", "urlFunction.file");
  assert.doesNotMatch(source, /^\s*import\s/m);
  assert.match(source, /export\s*\{[^}]*default/);
  assert.match(source, /https:\/\/example\.com\//);
});

test("imports outside the template folder are refused", async () => {
  const dir = folder({
    "functions/npm.ts": `import x from "left-pad";\nexport default () => x;\n`,
    "functions/node.ts": `import { readFileSync } from "node:fs";\nexport default () => readFileSync;\n`,
    "functions/up.ts": `import x from "../../outside";\nexport default () => x;\n`,
  });
  writeFileSync(join(dir, "..", "outside.ts"), "export default 1;\n");
  symlinkSync(join(dir, "..", "outside.ts"), join(dir, "functions", "link.ts"));
  for (const [file, pattern] of [
    ["./functions/npm.ts", /only relative imports/],
    ["./functions/node.ts", /only relative imports/],
    ["./functions/up.ts", /outside the template folder/],
    ["./functions/link.ts", /outside the template folder/],
    ["../outside.ts", /outside the template folder/],
    [join(dir, "functions", "npm.ts"), /absolute path/],
    ["./functions/missing.ts", /cannot find/],
  ] as const) {
    await assert.rejects(bundleFunction(dir, file, "f"), (e: unknown) => e instanceof TemplateError && pattern.test(e.message), file);
  }
});

test("template.json refers to files; inline sources are refused", async () => {
  const dir = folder({ "nodes/callout.ts": `export default () => "x";\n` });
  const out = await bundleFunctions(dir, {
    body: { customNodes: [{ name: "callout", toMarkdown: { file: "./nodes/callout.ts" } }] },
    contentTypes: [{ name: "post", hooks: { afterPublish: { file: "./nodes/callout.ts", permissions: ["http.fetch"], allowedHosts: ["a.example"] } } }],
  });
  const node = (out["body"] as { customNodes: { toMarkdown: { source: string } }[] }).customNodes[0]!;
  assert.match(node.toMarkdown.source, /"x"/);
  const hook = (out["contentTypes"] as { hooks: { afterPublish: Record<string, unknown> } }[])[0]!.hooks.afterPublish;
  assert.deepEqual(hook["allowedHosts"], ["a.example"]);
  assert.equal("file" in hook, false);
  await assert.rejects(bundleFunctions(dir, { contentTypes: [{ urlFunction: { source: "export default 1" } }] }), /refer to it with "file"/);
});

test("only text and richtext fields can be searchable (same rule as the server)", () => {
  const fields = [
    { name: "title", type: "text", searchable: true },
    { name: "body", type: "richtext", searchable: true },
    { name: "slug", type: "slug", searchable: true },
    { name: "related", type: "reference", searchable: false },
  ];
  assert.deepEqual(checkTemplateFunctions({ contentTypes: [{ fields }] }), [
    "contentTypes[0].fields[2].searchable: only text and richtext fields can be searchable",
  ]);
});

test("the CLI checks the same rules as the server", () => {
  assert.deepEqual(checkTemplateFunctions({ body: { customNodes: [{ name: "a", toMarkdown: { file: "./a.ts" } }] } }), []);
  const problems = checkTemplateFunctions({
    body: { customNodes: [{ name: "callout" }, { name: "b", toMarkdown: { file: "./b.ts", permissions: ["content.read"] } }] },
    contentTypes: [
      {
        urlFunction: { file: "./u.ts", permissions: ["http.fetch"], allowedHosts: ["a.example"] },
        hooks: {
          beforeSave: { file: "./s.ts", permissions: ["http.fetch"], allowedHosts: ["a.example"] },
          afterPublish: { file: "./p.ts", permissions: ["http.fetch"] },
          afterSave: { file: "./q.ts", allowedHosts: ["a.example"] },
        },
      },
    ],
  });
  for (const needle of [
    "customNodes[0].toMarkdown: a Markdown conversion function is required",
    "cms node scaffold callout",
    "customNodes[1].toMarkdown.permissions",
    "urlFunction.permissions",
    "hooks.beforeSave.permissions",
    "hooks.afterPublish.allowedHosts: list the hosts",
    "hooks.afterSave.allowedHosts: only for functions",
  ]) {
    assert.ok(problems.some((p) => p.includes(needle)), `${needle}: ${problems.join(" | ")}`);
  }
});

test("scaffold writes a conversion function that bundles, and prints the declaration", async () => {
  const dir = folder({ "template.json": JSON.stringify({ locales: ["ja"], defaultLocale: "ja", contentTypes: [] }) });
  const { file, declaration } = scaffoldNode(dir, "callout", { inline: false });
  assert.match(readFileSync(file, "utf8"), /export default function toMarkdown/);
  assert.deepEqual((declaration as { toMarkdown: unknown }).toMarkdown, { file: "./nodes/callout.ts" });
  assert.throws(() => scaffoldNode(dir, "callout", { inline: false }), /already exists/);
  assert.throws(() => scaffoldNode(dir, "bad-name", { inline: false }), TemplateError);
  // 宣言を足したテンプレートは、そのまま読み込める
  writeFileSync(
    join(dir, "template.json"),
    JSON.stringify({ locales: ["ja"], defaultLocale: "ja", body: { customNodes: [declaration] }, contentTypes: [] }),
  );
  const { manifest } = await loadTemplate(dir);
  const nodes = (manifest.body as unknown as { customNodes: { toMarkdown: { source: string } }[] }).customNodes;
  assert.doesNotMatch(nodes[0]!.toMarkdown.source, /^\s*import\s/m);
  // 宣言のない関数は、読み込む前に止まる
  writeFileSync(join(dir, "template.json"), JSON.stringify({ locales: ["ja"], defaultLocale: "ja", body: { customNodes: [{ name: "x" }] }, contentTypes: [] }));
  await assert.rejects(loadTemplate(dir), /a Markdown conversion function is required/);
});
