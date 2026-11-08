// 既存のブログからの取り込みの約束: INV-IMP-01〜03, INV-LOC-05
//
// 取り込みのCLI（Node.js）を、Markdownのフォルダに対して実行する。frontmatter のキーは
// title・slug・locale・url（元のURL）・translationKey（翻訳グループ）・translationOf（元の記事のslug）。

import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asSession, driver, type ImportOptions, type Session } from "./support/driver.js";
import { expectSuccess } from "./support/assert.js";
import { BASE_TEMPLATE, postType } from "./support/template.js";
import { lazyWorld, marker } from "./support/world.js";

const world = lazyWorld();

interface SourceDoc {
  file: string;
  title: string;
  slug: string;
  locale: string;
  url: string;
  translationKey?: string;
  translationOf?: string;
  body: string;
}

function writeSource(docs: SourceDoc[]): string {
  const dir = mkdtempSync(join(tmpdir(), "inv-import-"));
  for (const d of docs) {
    const fm = [`title: ${JSON.stringify(d.title)}`, `slug: ${d.slug}`, `locale: ${d.locale}`, `url: ${d.url}`];
    if (d.translationKey) fm.push(`translationKey: ${d.translationKey}`);
    if (d.translationOf) fm.push(`translationOf: ${d.translationOf}`);
    writeFileSync(join(dir, d.file), `---\n${fm.join("\n")}\n---\n\n${d.body}\n`);
  }
  return dir;
}

const newUrl = (locale: string, slug: string) => postType(BASE_TEMPLATE).urlTemplate.replace("{locale}", locale).replace("{slug}", slug);
const pathOf = (url: string) => new URL(url, "https://placeholder.invalid").pathname;

/** 取り込み先の新しいワークスペースと、取り込み用の個人トークン。 */
async function importTarget(): Promise<{ workspaceId: string; session: Session; token: string }> {
  const { yomi, serverAdmin, serverAdminSession } = await world();
  await yomi.reauthenticate(serverAdminSession, serverAdmin);
  const { id } = await yomi.createWorkspace(serverAdminSession, `inv-import-${randomUUID().slice(0, 6)}`);
  const admin = await yomi.createMember(serverAdminSession, id, "admin");
  const session = await yomi.login(admin);
  await yomi.reauthenticate(session, admin);
  const token = await yomi.createPersonalToken(session, [id], { scopes: ["read", "write"] });
  return { workspaceId: id, session, token: token.secret };
}

async function runImport(target: { workspaceId: string; token: string }, sourceDir: string, extra: Partial<ImportOptions> = {}) {
  const { yomi } = await world();
  const redirectOutput = join(sourceDir, `redirects-${randomUUID().slice(0, 6)}.json`);
  const result = await driver.runImport(yomi.baseUrl, {
    sourceDir,
    workspaceId: target.workspaceId,
    token: target.token,
    originalUrlKey: "url",
    translationKey: "translationKey",
    translationOf: "translationOf",
    redirectOutput,
    ...extra,
  });
  return { result, redirectOutput };
}

const doc = (label: string, locale = "ja", extra: Partial<SourceDoc> = {}): SourceDoc => {
  const slug = `${label}-${randomUUID().slice(0, 8)}`;
  return { file: `${slug}.md`, title: marker(label), slug, locale, url: `https://old.inv.test/${locale}/${slug}.html`, body: `本文 ${marker(label)}`, ...extra };
};

inv("inv_imp_01", "対応が設定されていない独自部品を含むMarkdownは、取り込み前にエラーで止まり、一部だけが取り込まれることはない", async () => {
  const { yomi } = await world();
  const target = await importTarget();
  const plain1 = doc("imp01-a");
  const plain2 = doc("imp01-b");
  const withComponent = doc("imp01-c", "ja", { body: `前置き\n\n<Callout type="warning">注意</Callout>\n\n後書き` });
  const source = writeSource([plain1, plain2, withComponent]);

  const { result } = await runImport(target, source, { componentMap: {} });
  expect(result.exitCode, "対応のない独自部品があるのに、取り込みが成功した").not.toBe(0);
  expect(await yomi.listEntries(target.session, target.workspaceId), "一部の記事だけが取り込まれた").toEqual([]);

  // 対照: 独自部品のないファイルだけなら取り込める
  const ok = await runImport(target, writeSource([plain1, plain2]), { componentMap: {} });
  expect(ok.result.exitCode, `独自部品のない取り込みが失敗した:\n${ok.result.output}`).toBe(0);
  expect((await yomi.listEntries(target.session, target.workspaceId)).length).toBe(2);
});

inv("inv_imp_02", "取り込んだ記事の元のURLは、新しいURLとの対応としてURLの変更履歴に記録される", async () => {
  const { yomi } = await world();
  const target = await importTarget();
  const docs = [doc("imp02-a"), doc("imp02-b", "en"), doc("imp02-c")];
  const { result } = await runImport(target, writeSource(docs));
  expect(result.exitCode, `取り込みが失敗した:\n${result.output}`).toBe(0);

  const entries = await yomi.listEntries(target.session, target.workspaceId);
  for (const d of docs) {
    const entry = entries.find((e) => e.slug === d.slug);
    expect(entry, `${d.file} が取り込まれていない`).toBeDefined();
    const history = await yomi.getUrlHistory(target.session, target.workspaceId, entry!.id);
    expect(history, `${d.file} の元のURLが変更履歴にない`).toContainEqual({ from: d.url, to: newUrl(d.locale, d.slug) });
  }
});

inv("inv_imp_03", "取り込みCLIが書き出すリダイレクト設定には、取り込んだすべての記事の元のURLから新しいURLへの対応が含まれる", async () => {
  const target = await importTarget();
  const docs = [doc("imp03-a"), doc("imp03-b", "en"), doc("imp03-c", "fr"), doc("imp03-d")];
  const { result, redirectOutput } = await runImport(target, writeSource(docs));
  expect(result.exitCode, `取り込みが失敗した:\n${result.output}`).toBe(0);

  const redirects = (await driver.readRedirectFile(redirectOutput)).map((r) => ({ from: pathOf(r.from), to: pathOf(r.to) }));
  for (const d of docs) {
    expect(redirects, `${d.file} のリダイレクトがない`).toContainEqual({ from: pathOf(d.url), to: pathOf(newUrl(d.locale, d.slug)) });
  }
});

inv(
  "inv_loc_05",
  "取り込みで翻訳グループにまとめた記事には、既定のロケールの記事（取り込みの設定で元の記事を指定した場合はその記事）を元の記事とし、取り込んだ時点の版を反映済みとする記録が残る（元の記事が決まらないグループは「元なし」）",
  async () => {
    const { yomi } = await world();
    const target = await importTarget();
    expect(BASE_TEMPLATE.defaultLocale).toBe("ja");
    const key = () => `g-${randomUUID().slice(0, 8)}`;

    // g1: ja（既定のロケール）と en → 元は ja
    const g1 = key();
    const g1ja = doc("loc05-g1-ja", "ja", { translationKey: g1 });
    const g1en = doc("loc05-g1-en", "en", { translationKey: g1 });
    // g2: en と fr だけ → 元なし
    const g2 = key();
    const g2en = doc("loc05-g2-en", "en", { translationKey: g2 });
    const g2fr = doc("loc05-g2-fr", "fr", { translationKey: g2 });
    // g3: ja・en・fr で、元の記事を en に指定 → 元は en
    const g3 = key();
    const g3en = doc("loc05-g3-en", "en", { translationKey: g3 });
    const g3ja = doc("loc05-g3-ja", "ja", { translationKey: g3, translationOf: g3en.slug });
    const g3fr = doc("loc05-g3-fr", "fr", { translationKey: g3, translationOf: g3en.slug });
    const all = [g1ja, g1en, g2en, g2fr, g3en, g3ja, g3fr];

    const { result } = await runImport(target, writeSource(all));
    expect(result.exitCode, `取り込みが失敗した:\n${result.output}`).toBe(0);
    const entries = await yomi.listEntries(target.session, target.workspaceId);
    const idOf = (d: SourceDoc) => entries.find((e) => e.slug === d.slug)!.id;
    const info = async (d: SourceDoc) => yomi.getTranslationInfo(target.session, target.workspaceId, idOf(d));
    /** 元の記事の、取り込んだ時点の版（取り込みの直後なので、唯一の版）。 */
    const importedVersion = async (d: SourceDoc) => {
      const versions = await yomi.listVersions(target.session, target.workspaceId, idOf(d));
      expect(versions.length, `${d.file} の版の数`).toBe(1);
      return versions[0]!.id;
    };

    // グループのまとまり
    const groups = new Map<string, string>();
    for (const d of all) groups.set(d.file, (await info(d)).groupId);
    expect(groups.get(g1ja.file)).toBe(groups.get(g1en.file));
    expect(groups.get(g2en.file)).toBe(groups.get(g2fr.file));
    expect(new Set([groups.get(g3en.file), groups.get(g3ja.file), groups.get(g3fr.file)]).size).toBe(1);
    expect(new Set([groups.get(g1ja.file), groups.get(g2en.file), groups.get(g3en.file)]).size, "別のグループがまとめられた").toBe(3);

    // g1: en の元は ja、反映済みは取り込んだ時点の ja の版
    const g1enInfo = await info(g1en);
    expect(g1enInfo.sourceEntryId).toBe(idOf(g1ja));
    expect(g1enInfo.reflectedSourceVersionId).toBe(await importedVersion(g1ja));
    expect((await info(g1ja)).sourceEntryId, "元の記事自身に元が記録されている").toBeNull();

    // g2: 元なし
    for (const d of [g2en, g2fr]) {
      const i = await info(d);
      expect(i.sourceEntryId, `${d.file} は元なしのはず`).toBeNull();
      expect(i.reflectedSourceVersionId).toBeNull();
    }

    // g3: 指定した en が元（既定のロケールの ja より優先）
    for (const d of [g3ja, g3fr]) {
      const i = await info(d);
      expect(i.sourceEntryId, `${d.file} の元の記事`).toBe(idOf(g3en));
      expect(i.reflectedSourceVersionId).toBe(await importedVersion(g3en));
    }
    expect((await info(g3en)).sourceEntryId).toBeNull();
    expectSuccess(await yomi.getEntryRaw(asSession(target.session), target.workspaceId, idOf(g3en)), "取り込んだ記事の取得");
  },
  180_000,
);
