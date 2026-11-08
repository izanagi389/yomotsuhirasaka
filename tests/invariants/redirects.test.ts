// リダイレクトの約束: INV-RED-01, INV-RED-02
//
// リダイレクト一覧は配信APIで取る（driver.redirects.parse）。URLはスラッグを変えて公開し直すことで変える。
// 一覧には、同じワークスペースの他のテストの記事も入るので、このテストで作った記事のURLだけを見る。

import { randomUUID } from "node:crypto";
import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { driver, type RedirectRule } from "./support/driver.js";
import { expectSuccess } from "./support/assert.js";
import { simpleBody } from "./support/body.js";
import { lazyWorld, marker } from "./support/world.js";

const world = lazyWorld();
const slug = (label: string) => `${label}-${randomUUID().slice(0, 8)}`;

async function redirects(): Promise<RedirectRule[]> {
  const { yomi, a } = await world();
  const query = await yomi.registerQuery(a.id, a.registerKey.secret, { kind: "redirects" });
  const res = await yomi.deliveryRaw(a.id, a.readKey.secret, query);
  expectSuccess(res, "リダイレクト一覧の取得");
  return driver.redirects.parse(res.text);
}

/** 記事を作って公開し、スラッグを順に変えて公開し直す。公開したURLを順に返す。 */
async function publishThrough(slugs: string[]): Promise<{ id: string; urls: string[] }> {
  const { yomi, a } = await world();
  const s = a.editorSession;
  const { id } = await yomi.createEntry(s, a.id, { title: marker("red"), slug: slugs[0]!, body: simpleBody("本文") });
  await yomi.publishEntry(s, a.id, id);
  for (const next of slugs.slice(1)) {
    await yomi.saveDraft(s, a.id, id, { slug: next });
    await yomi.publishEntry(s, a.id, id);
  }
  // 公開したURLは、変更履歴の from と最後の to から読む（URLの組み立て方に依存しないため）
  const history = await yomi.getUrlHistory(s, a.id, id);
  expect(history.length, "スラッグを変えて公開し直したのに、URLの変更履歴がない").toBe(slugs.length - 1);
  return { id, urls: [...history.map((h) => h.from), history[history.length - 1]!.to] };
}

const ruleFor = (rules: RedirectRule[], from: string) => rules.filter((r) => r.from === from);

inv("inv_red_01", "リダイレクト一覧に連鎖はなく、すべての旧URLは最終的なURLへ直接転送される", async () => {
  // A → B → C と2回変わった記事
  const chain = await publishThrough([slug("red01-a"), slug("red01-b"), slug("red01-c")]);
  const [a, b, c] = chain.urls as [string, string, string];
  // X → Y → X と、元のURLに戻った記事
  const back = await publishThrough([slug("red01-x"), slug("red01-y")]);
  const [x, y] = back.urls as [string, string];
  const { yomi, a: ws } = await world();
  await yomi.saveDraft(ws.editorSession, ws.id, back.id, { slug: x.split("/").pop()! });
  await yomi.publishEntry(ws.editorSession, ws.id, back.id);

  const rules = await redirects();
  // すべての旧URLは、最終的なURLへ直接転送される
  for (const old of [a, b]) {
    expect(ruleFor(rules, old), `旧URL ${old} の転送`).toEqual([{ from: old, to: c, status: 301 }]);
  }
  expect(ruleFor(rules, y), `旧URL ${y} の転送`).toEqual([{ from: y, to: x, status: 301 }]);
  // いま公開中のURLは転送しない（元のURLに戻った場合も）
  for (const live of [c, x]) {
    expect(ruleFor(rules, live), `公開中のURL ${live} が転送されている`).toEqual([]);
  }
  // 一覧のどこにも連鎖がない: 転送先が、別の転送や「消えた」の転送元になっていない
  const froms = new Set(rules.map((r) => r.from));
  for (const r of rules.filter((r) => r.to !== null)) {
    expect(froms.has(r.to!), `連鎖がある: ${r.from} → ${r.to} → …`).toBe(false);
  }
  // 転送元は1つのURLにつき1件
  expect(froms.size, "同じ転送元が一覧に何度も出ている").toBe(rules.length);
});

inv("inv_red_02", "削除した記事のURLは、リダイレクト一覧に「消えた（410）」として含まれる", async () => {
  const { yomi, a } = await world();
  const gone = await publishThrough([slug("red02-a"), slug("red02-b")]);
  const [oldUrl, lastUrl] = gone.urls as [string, string];

  // 対照: 削除する前は、旧URLは転送され、公開中のURLは一覧にない
  let rules = await redirects();
  expect(ruleFor(rules, oldUrl), "削除する前の旧URLの転送").toEqual([{ from: oldUrl, to: lastUrl, status: 301 }]);
  expect(ruleFor(rules, lastUrl), "削除する前の公開中のURL").toEqual([]);

  await yomi.deleteEntry(a.editorSession, a.id, gone.id);
  rules = await redirects();
  // 削除した記事の、最後のURLも過去のURLも「消えた（410）」
  for (const url of [lastUrl, oldUrl]) {
    expect(ruleFor(rules, url), `削除した記事のURL ${url}`).toEqual([{ from: url, to: null, status: 410 }]);
  }
});
