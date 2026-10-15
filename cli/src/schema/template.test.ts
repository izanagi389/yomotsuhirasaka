import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { TemplateError, loadTemplate, templateName, toManifest } from "./template.js";

const template = {
  $schema: "../../node_modules/@new-cms/cli/schema/template.schema.json",
  locales: ["ja", "en"],
  defaultLocale: "ja",
  body: { embedProviders: ["www.youtube.com"] },
  contentTypes: [{ name: "post", urlTemplate: "https://example.com/{locale}/{slug}", listLimit: 10, fields: [{ name: "slug", type: "slug" }] }],
};

test("the manifest gets the body's nodes and marks from the editor's extensions", () => {
  const m = toManifest(template);
  assert.equal("$schema" in m, false);
  assert.deepEqual(m.body.embedProviders, ["www.youtube.com"]);
  const nodes = m.body.nodes as { name: string; attrs: string[] }[];
  assert.ok(nodes.some((n) => n.name === "doc"));
  assert.deepEqual(nodes.find((n) => n.name === "image")?.attrs, ["mediaId", "alt", "decorative"]);
  assert.ok((m.body.marks as { name: string }[]).some((k) => k.name === "link"));
  assert.deepEqual(m.contentTypes, template.contentTypes);
});

test("body.nodes and body.marks are not written by hand", () => {
  assert.throws(() => toManifest({ ...template, body: { nodes: [] } }), TemplateError);
  assert.throws(() => toManifest([]), TemplateError);
  // body を省けば、埋め込みの提供元はなし
  assert.deepEqual(toManifest({ ...template, body: undefined }).body.embedProviders, []);
});

test("the folder name is the template name", () => {
  assert.equal(templateName("templates/blog"), "blog");
  assert.throws(() => templateName("templates/Blog"), TemplateError);
  const root = mkdtempSync(join(tmpdir(), "cms-template-"));
  const dir = join(root, "blog");
  mkdirSync(dir);
  writeFileSync(join(dir, "template.json"), JSON.stringify(template));
  assert.equal(loadTemplate(dir).name, "blog");
  writeFileSync(join(dir, "template.json"), "{ not json");
  assert.throws(() => loadTemplate(dir), /not valid JSON/);
});
