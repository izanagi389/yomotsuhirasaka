// 組み込みJSの約束: INV-HOOK-02〜04
//
// 組み込みJSで動く関数の結果を外から確かめるため、ここではURL生成関数を使う（関数の返したURLが、
// 配信APIとURLの変更履歴に出る）。フック（保存前・公開後など）での確認は、フックの受け取る値と返す値を
// 決めた後に、INV-HOOK-01・05 のテストと一緒に足す。
//
// テンプレートのデプロイはテンプレートを使うすべてのワークスペースに効くため、テストごとに別の名前の
// テンプレートを使う。関数のソースは support/scripts.ts にある。

import { randomUUID } from "node:crypto";
import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asBearer, asSession, driver, type CmsClient, type Credentials, type IssuedSecret, type RegisteredQuery, type Session, type StartedServer } from "./support/driver.js";
import { expectNotLeaked, expectSuccess, expectTextNotLeaked } from "./support/assert.js";
import type { HttpResponse } from "./support/http.js";
import { simpleBody } from "./support/body.js";
import { readingUrlFunction, RUNAWAY_FINISHED, runawayUrlFunction } from "./support/scripts.js";
import { cloneTemplate, postType, type ScriptSpec, type TemplateSpec } from "./support/template.js";
import { lazyWorld, marker, sleep } from "./support/world.js";

const world = lazyWorld();
const templateName = (label: string) => `inv-${label}-${randomUUID().slice(0, 8)}`;
const withUrlFunction = (name: string, fn?: ScriptSpec): TemplateSpec => cloneTemplate(name, (t) => (postType(t).urlFunction = fn));

/** 公開URLがテストのサイトのURLとして計算済みであることの目印（配信APIの応答の中）。 */
const COMPUTED_URL = /https:\/\/site\.inv\.test\/ja\/blog\//;

interface Site {
  cms: CmsClient;
  id: string;
  editorSession: Session;
  readKey: IssuedSecret;
  urlQuery: RegisteredQuery;
}

/** テンプレートを使うワークスペースを作り、公開URLを読むクエリを登録しておく。 */
async function siteOn(cms: CmsClient, serverAdmin: Credentials, serverAdminSession: Session, template: string): Promise<Site> {
  await cms.reauthenticate(serverAdminSession, serverAdmin);
  const { id } = await cms.createWorkspace(serverAdminSession, `${template}-ws-${randomUUID().slice(0, 6)}`, { template });
  const admin = await cms.createMember(serverAdminSession, id, "admin");
  const adminSession = await cms.login(admin);
  const editor = await cms.createMember(adminSession, id, "editor");
  const editorSession = await cms.login(editor);
  await cms.reauthenticate(adminSession, admin);
  const readKey = await cms.createApiKey(adminSession, id, ["read"]);
  const registerKey = await cms.createApiKey(adminSession, id, ["register"]);
  const urlQuery = await cms.registerQuery(id, registerKey.secret, { kind: "byId", fields: ["url"] });
  return { cms, id, editorSession, readKey, urlQuery };
}

/** 記事を作って公開し、IDを返す。タイトルはURLに入れられる文字だけにする。 */
async function published(site: Site, title: string, slug?: string): Promise<string> {
  const { id } = await site.cms.createEntry(site.editorSession, site.id, { title, slug, body: simpleBody("本文") });
  await site.cms.publishEntry(site.editorSession, site.id, id);
  return id;
}

/**
 * 記事の公開URLを配信APIで読む。URLの計算を後から行う実装もありうるので、計算済みになるまで待つ。
 * 待っても計算済みにならなければ、最後の応答をそのまま返す（公開や関数の実行が失敗した場合）。
 */
async function settledUrl(site: Site, entryId: string, waitSeconds = 30): Promise<HttpResponse> {
  const deadline = Date.now() + waitSeconds * 1000;
  for (;;) {
    const res = await site.cms.deliveryRaw(site.id, site.readKey.secret, site.urlQuery, { id: entryId, locale: "ja" });
    if (COMPUTED_URL.test(res.text) || Date.now() > deadline) return res;
    await sleep(0.5);
  }
}

/** 記事の公開URL（配信API）とURLの変更履歴を、まとめて文字列にする。 */
async function urlTrail(site: Site, entryId: string): Promise<string> {
  const current = await settledUrl(site, entryId);
  const history = await site.cms.getUrlHistory(site.editorSession, site.id, entryId);
  return [current.text, ...history.flatMap((h) => [h.from, h.to])].join("\n");
}

inv("inv_hook_02", "フックやURL生成関数は、権限として宣言していないホスト関数を呼べない", async () => {
  const { cms, serverAdmin, serverAdminSession } = await world();
  const deploy = async (t: TemplateSpec) => expectSuccess(await cms.deployTemplateRaw(asBearer(await cms.deployToken(serverAdminSession, serverAdmin)), t), "テンプレートのデプロイ");
  const name = templateName("hook02");
  await deploy(withUrlFunction(name));
  const site = await siteOn(cms, serverAdmin, serverAdminSession, name);
  const targetTitle = marker("hook02");
  const target = await published(site, targetTitle);

  // 記事を読む関数を、記事を読む権限を宣言せずにデプロイする
  await deploy(withUrlFunction(name, { source: readingUrlFunction([target]), permissions: [] }));
  const { id: undeclared } = await cms.createEntry(site.editorSession, site.id, { title: marker("hook02-probe"), body: simpleBody("本文") });
  // 呼び出しが例外になって関数が失敗し、公開が止まる実装もありうるので、公開の成否は問わない
  await cms.publishRaw(asSession(site.editorSession), site.id, undeclared);
  for (const [what, entryId] of [["権限を宣言していない関数で計算した記事", undeclared], ["読もうとした記事", target]] as const) {
    expectTextNotLeaked(await urlTrail(site, entryId), targetTitle, `${what}の公開URLと変更履歴`);
  }
  expectNotLeaked(await settledUrl(site, undeclared), targetTitle, "権限を宣言していない関数で計算した記事の配信");

  // 対照: 同じ関数でも、権限を宣言すれば記事を読める（関数は実際に動いている）
  await deploy(withUrlFunction(name, { source: readingUrlFunction([target]), permissions: ["readContent"] }));
  const declared = await published(site, marker("hook02-control"));
  expect((await settledUrl(site, declared)).text.includes(targetTitle), "権限を宣言した関数が記事を読めていない（関数が動いていない）").toBe(true);
}, 180_000);

inv("inv_hook_03", "フックやURL生成関数からは、他のワークスペースのデータを読めない", async () => {
  const { cms, serverAdmin, serverAdminSession } = await world();
  const deploy = async (t: TemplateSpec) => expectSuccess(await cms.deployTemplateRaw(asBearer(await cms.deployToken(serverAdminSession, serverAdmin)), t), "テンプレートのデプロイ");
  const name = templateName("hook03");
  const other = templateName("hook03-other");
  await deploy(withUrlFunction(name));
  await deploy(withUrlFunction(other));

  // 同じテンプレート（同じ関数）を使う2つのワークスペースと、別のテンプレートのワークスペース
  const mine = await siteOn(cms, serverAdmin, serverAdminSession, name);
  const sibling = await siteOn(cms, serverAdmin, serverAdminSession, name);
  const stranger = await siteOn(cms, serverAdmin, serverAdminSession, other);
  const mineTitle = marker("hook03-mine");
  const siblingTitle = marker("hook03-sibling");
  const strangerTitle = marker("hook03-stranger");
  const ids = [await published(mine, mineTitle), await published(sibling, siblingTitle), await published(stranger, strangerTitle)];

  // 3つの記事を読み、前回の実行の結果をグローバル変数に残す関数。読む権限は宣言している
  await deploy(withUrlFunction(name, { source: readingUrlFunction(ids), permissions: ["readContent"] }));
  // 同じ関数を、先に隣のワークスペースで動かしてから、自分のワークスペースで動かす
  const siblingProbe = await published(sibling, marker("hook03-sibling-probe"));
  expect((await settledUrl(sibling, siblingProbe)).text.includes(siblingTitle), "隣のワークスペースで、関数が自分の記事を読めていない").toBe(true);
  const probe = await published(mine, marker("hook03-probe"));

  // 対照: 自分のワークスペースの記事は読める
  expect((await settledUrl(mine, probe)).text.includes(mineTitle), "関数が自分のワークスペースの記事を読めていない（関数が動いていない）").toBe(true);

  // 自分のワークスペースのどの記事の公開URLにも、他のワークスペースの記事のタイトルが出ない
  for (const entryId of [ids[0]!, probe]) {
    const trail = await urlTrail(mine, entryId);
    expectTextNotLeaked(trail, siblingTitle, "同じテンプレートの別のワークスペースの記事（IDを指定した読み取りと、残されたグローバル変数）");
    expectTextNotLeaked(trail, strangerTitle, "別のテンプレートのワークスペースの記事");
  }
}, 180_000);

inv("inv_hook_04", "メモリや実行時間の上限を超えたフックは打ち切られ、サーバーの他の処理（他のリクエストや配信）は続く", async () => {
  // 上限を小さくしたサーバーで確かめる。上限がなければ、関数は spinMs 後と bombMegabytes を確保した後に実行を終える
  const limits = { memoryBytes: 32 * 1024 * 1024, timeMs: 1_000 };
  const spinMs = 20_000;
  const instance = await driver.createIsolatedInstance();
  const server: StartedServer = await driver.startServer({ ...instance.config, scriptLimits: limits });
  expect(server.outcome, `サーバーが起動しない:\n${server.output}`).toBe("ready");
  try {
    const cms = server.client!;
    const admin = instance.serverAdmin;
    const adminSession = await cms.login(admin);
    const name = templateName("hook04");
    const fn: ScriptSpec = { source: runawayUrlFunction({ spinMs, bombMegabytes: 256 }), permissions: [] };
    expectSuccess(await cms.deployTemplateRaw(asBearer(await cms.deployToken(adminSession, admin)), withUrlFunction(name, fn)), "テンプレートのデプロイ");
    const site = await siteOn(cms, admin, adminSession, name);

    // 対照: ふつうの記事では関数が動き、URLが計算される
    const ok = await published(site, marker("hook04-ok"), `ok-${randomUUID().slice(0, 8)}`);
    expect((await settledUrl(site, ok)).text.includes("/blog/ok/"), "上限の内側の関数でURLが計算されない").toBe(true);

    // 実行時間: 計算を続ける関数が動いている間も、配信と管理APIは応答を続ける
    const { id: spin } = await cms.createEntry(site.editorSession, site.id, { title: marker("hook04-spin"), slug: `spin-${randomUUID().slice(0, 8)}`, body: simpleBody("本文") });
    const started = Date.now();
    const spinPublish = cms.publishRaw(asSession(site.editorSession), site.id, spin);
    await sleep(0.3);
    for (let i = 0; i < 5; i++) {
      const t0 = performance.now();
      expectSuccess(await cms.deliveryRaw(site.id, site.readKey.secret, site.urlQuery, { id: ok, locale: "ja" }), "上限を超えた関数の実行中の配信");
      expectSuccess(await cms.meRaw(asSession(site.editorSession)), "上限を超えた関数の実行中の管理API");
      expect(performance.now() - t0, "上限を超えた関数の実行中に、配信と管理APIの応答が遅れた").toBeLessThan(2_000);
      await sleep(0.3);
    }
    await spinPublish; // 公開が成功するか失敗するかは問わない。応答が返ること（下の待ち時間の中で）を確かめる
    // 上限がなければ関数が実行を終えている時刻まで待ってから、関数が最後まで動いていないことを確かめる
    await sleep(Math.max(0, (spinMs + 5_000 - (Date.now() - started)) / 1000));
    expectTextNotLeaked(await urlTrail(site, spin), RUNAWAY_FINISHED, "実行時間の上限を超えた関数の結果");

    // メモリ: 大量に確保する関数は打ち切られ、サーバーは動き続ける
    const { id: bomb } = await cms.createEntry(site.editorSession, site.id, { title: marker("hook04-bomb"), slug: `bomb-${randomUUID().slice(0, 8)}`, body: simpleBody("本文") });
    await cms.publishRaw(asSession(site.editorSession), site.id, bomb);
    await sleep(10);
    expectTextNotLeaked(await urlTrail(site, bomb), RUNAWAY_FINISHED, "メモリの上限を超えた関数の結果");

    // 打ち切りの後も、サーバーは同じように動く
    const after = await published(site, marker("hook04-after"), `ok-${randomUUID().slice(0, 8)}`);
    expect((await settledUrl(site, after)).text.includes("/blog/ok/"), "打ち切りの後に、ふつうの記事のURLが計算されない").toBe(true);
    expectSuccess(await cms.deliveryRaw(site.id, site.readKey.secret, site.urlQuery, { id: ok, locale: "ja" }), "打ち切りの後の配信");
  } finally {
    await server.stop();
  }
}, 300_000);
