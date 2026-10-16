// 組み込みJSの約束: INV-HOOK-01〜05
//
// 組み込みJSで動く関数の結果は、次のところで外から確かめる。
// - URL生成関数: 関数の返したURL（配信APIとURLの変更履歴）
// - 保存前のフック: フックが直したフィールド（下書き）
// - 公開前のフック: フックが止めた公開
// - 非同期のフック: テストの受信サーバーに届いたリクエスト
//
// テンプレートのデプロイはテンプレートを使うすべてのワークスペースに効くため、テストごとに別の名前の
// テンプレートを使う。関数のソースは support/scripts.ts にある。
// 受信サーバーとDNSサーバーの使い方は webhooks.test.ts と同じ（env.harnessAllowedHost は送信の例外として許可済み、
// env.harnessBlockedHost は許可していない内部のアドレス）。

import { randomUUID } from "node:crypto";
import { afterAll, expect } from "vitest";
import { inv } from "./support/inv.js";
import { asBearer, asSession, driver, type CmsClient, type Credentials, type IssuedSecret, type RegisteredQuery, type Session, type StartedServer } from "./support/driver.js";
import { expectNotLeaked, expectRefused, expectSuccess, expectTextNotLeaked } from "./support/assert.js";
import type { HttpResponse } from "./support/http.js";
import { simpleBody } from "./support/body.js";
import { startDnsServer, type DnsServer } from "./support/dns.js";
import { env } from "./support/env.js";
import { startReceiver, type Receiver, type ReceivedRequest } from "./support/net.js";
import {
  afterSending,
  beforePublishSending,
  beforeSaveReading,
  beforeSaveSending,
  readingUrlFunction,
  RUNAWAY_FINISHED,
  runawayAfterPublish,
  runawayBeforeSave,
  runawayUrlFunction,
} from "./support/scripts.js";
import { cloneTemplate, postType, type HookKind, type ScriptSpec, type TemplateSpec } from "./support/template.js";
import { lazyWorld, marker, sleep } from "./support/world.js";

const world = lazyWorld();
const templateName = (label: string) => `inv-${label}-${randomUUID().slice(0, 8)}`;
const withUrlFunction = (name: string, fn?: ScriptSpec): TemplateSpec => cloneTemplate(name, (t) => (postType(t).urlFunction = fn));
const withHooks = (name: string, hooks: Partial<Record<HookKind, ScriptSpec>>, urlFunction?: ScriptSpec): TemplateSpec =>
  cloneTemplate(name, (t) => {
    postType(t).hooks = hooks;
    postType(t).urlFunction = urlFunction;
  });

let receiverPromise: Promise<Receiver> | null = null;
let dnsPromise: Promise<DnsServer> | null = null;
const receiver = () => (receiverPromise ??= startReceiver());
const dns = () => (dnsPromise ??= startDnsServer(env.harnessDnsPort));
afterAll(async () => {
  if (receiverPromise) await (await receiverPromise).close();
  if (dnsPromise) await (await dnsPromise).close();
});
const uniquePath = (label: string) => `/${label}-${randomUUID().slice(0, 8)}`;
const uniqueName = (label: string) => `${label}-${randomUUID().slice(0, 8)}.inv.test`;
/** 非同期のフックが、記事のIDを付けて path に送ったリクエスト。 */
const sentFor = (path: string, entryId: string) => (r: ReceivedRequest) => r.path.startsWith(path) && r.body.includes(entryId);

/** テストの舞台のCMSに、デプロイ用のトークンでテンプレートをデプロイする（応答をそのまま返す）。 */
async function deployRaw(t: TemplateSpec): Promise<HttpResponse> {
  const { cms, serverAdmin, serverAdminSession } = await world();
  return cms.deployTemplateRaw(asBearer(await cms.deployToken(serverAdminSession, serverAdmin)), t);
}
const deploy = async (t: TemplateSpec) => expectSuccess(await deployRaw(t), "テンプレートのデプロイ");

/** 記事の下書きを保存する（保存前のフックを動かす）。保存の成否は問わない。 */
async function saveRaw(site: Site, entryId: string, fields: Record<string, unknown>): Promise<HttpResponse> {
  const draft = await site.cms.getDraft(site.editorSession, site.id, entryId);
  return site.cms.saveDraftRaw(asSession(site.editorSession), site.id, entryId, { fields }, draft.revision);
}
const subtitleOf = async (site: Site, entryId: string): Promise<unknown> => (await site.cms.getDraft(site.editorSession, site.id, entryId)).fields["subtitle"];

/** 関数が上限を超えて動いている間も、配信と管理APIが続けて応答することを確かめる。 */
async function expectStillServing(site: Site, publishedEntryId: string, what: string): Promise<void> {
  for (let i = 0; i < 5; i++) {
    const t0 = performance.now();
    expectSuccess(await site.cms.deliveryRaw(site.id, site.readKey.secret, site.urlQuery, { id: publishedEntryId, locale: "ja" }), `${what}の間の配信`);
    expectSuccess(await site.cms.meRaw(asSession(site.editorSession)), `${what}の間の管理API`);
    expect(performance.now() - t0, `${what}の間に、配信と管理APIの応答が遅れた`).toBeLessThan(2_000);
    await sleep(0.3);
  }
}

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

inv("inv_hook_01", "同期フックからは外部と通信できない", async () => {
  const { cms, serverAdmin, serverAdminSession } = await world();
  const rc = await receiver();
  const host = env.harnessAllowedHost;
  const name = templateName("hook01");

  // デプロイ: 同期のフックには、外部へ送る権限を宣言できない（CLIを通さず、デプロイのAPIを直接呼ぶ）
  const declared = rc.url(host, uniquePath("hook01-declared"));
  for (const [kind, source] of [
    ["beforeSave", beforeSaveSending([declared], "x")],
    ["beforePublish", beforePublishSending([declared])],
  ] as const) {
    expectRefused(await deployRaw(withHooks(name, { [kind]: { source, permissions: ["fetch"], allowedHosts: [host] } })), `外部へ送る権限を宣言した${kind}のフック`);
  }

  // 実行: 同期のフックは、送ろうとしても送れない。非同期のフックからは同じ宛先に届く（対照）
  const syncPath = uniquePath("hook01-sync");
  const asyncPath = uniquePath("hook01-async");
  const tag = marker("hook01-ran");
  await deploy(
    withHooks(name, {
      beforeSave: { source: beforeSaveSending([rc.url(host, `${syncPath}-save`)], tag), permissions: [] },
      beforePublish: { source: beforePublishSending([rc.url(host, `${syncPath}-publish`)]), permissions: [] },
      afterPublish: { source: afterSending([rc.url(host, asyncPath)]), permissions: ["fetch"], allowedHosts: [host] },
    }),
  );
  const site = await siteOn(cms, serverAdmin, serverAdminSession, name);
  const { id } = await cms.createEntry(site.editorSession, site.id, { title: marker("hook01"), body: simpleBody("本文") });
  expectSuccess(await saveRaw(site, id, { summary: "要約" }), "下書きの保存");
  expect(await subtitleOf(site, id), "保存前のフックが動いていない").toBe(tag);
  // 公開前のフックが動いていること: タイトルが block- の記事の公開を止める
  const { id: blocked } = await cms.createEntry(site.editorSession, site.id, { title: `block-${randomUUID().slice(0, 8)}`, body: simpleBody("本文") });
  expectRefused(await cms.publishRaw(asSession(site.editorSession), site.id, blocked), "公開前のフックが止めた記事の公開");
  await cms.publishEntry(site.editorSession, site.id, id);

  await rc.waitFor(sentFor(asyncPath, id), 60_000);
  await sleep(5);
  expect(rc.requests.filter((r) => r.path.startsWith(syncPath)).map((r) => r.path), "同期のフックから外部に送られた").toEqual([]);
}, 180_000);

inv("inv_hook_02", "フックやURL生成関数は、権限として宣言していないホスト関数を呼べない", async () => {
  const { cms, serverAdmin, serverAdminSession } = await world();
  const name = templateName("hook02");
  await deploy(withUrlFunction(name));
  const site = await siteOn(cms, serverAdmin, serverAdminSession, name);
  const targetTitle = marker("hook02");
  const target = await published(site, targetTitle);

  // URL生成関数: 記事を読む関数を、記事を読む権限を宣言せずにデプロイする
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

  // 保存前のフック: 記事を読む権限を宣言していなければ読めない
  await deploy(withHooks(name, { beforeSave: { source: beforeSaveReading([target]), permissions: [] } }));
  const { id: saved } = await cms.createEntry(site.editorSession, site.id, { title: marker("hook02-save"), body: simpleBody("本文") });
  await saveRaw(site, saved, { summary: "要約" });
  expectTextNotLeaked(String(await subtitleOf(site, saved)), targetTitle, "権限を宣言していない保存前のフックが直した下書き");
  // 対照: 宣言すれば読める
  await deploy(withHooks(name, { beforeSave: { source: beforeSaveReading([target]), permissions: ["readContent"] } }));
  expectSuccess(await saveRaw(site, saved, { summary: "要約2" }), "下書きの保存");
  expect(String(await subtitleOf(site, saved)).includes(targetTitle), "権限を宣言した保存前のフックが記事を読めていない").toBe(true);

  // 非同期のフック: 外部へ送る権限を宣言していなければ送れない。宣言していても、宣言していない宛先には送れない
  const rc = await receiver();
  const host = env.harnessAllowedHost;
  const undeclaredPath = uniquePath("hook02-undeclared");
  const unlistedPath = uniquePath("hook02-unlisted");
  const controlPath = uniquePath("hook02-control");
  await deploy(withHooks(name, { afterPublish: { source: afterSending([rc.url(host, undeclaredPath)]), permissions: [] } }));
  const e1 = await published(site, marker("hook02-after1"));
  await deploy(withHooks(name, { afterPublish: { source: afterSending([rc.url(host, unlistedPath)]), permissions: ["fetch"], allowedHosts: ["other.inv.test"] } }));
  const e2 = await published(site, marker("hook02-after2"));
  // 対照: 権限と宛先を宣言すれば届く（前の2つの記事のフックも、この時点までに動いている）
  await deploy(withHooks(name, { afterPublish: { source: afterSending([rc.url(host, controlPath)]), permissions: ["fetch"], allowedHosts: [host] } }));
  const e3 = await published(site, marker("hook02-after3"));
  await rc.waitFor(sentFor(controlPath, e3), 60_000);
  await sleep(5);
  expect(rc.requests.filter((r) => sentFor(undeclaredPath, e1)(r)).length, "外部へ送る権限を宣言していないフックから送られた").toBe(0);
  expect(rc.requests.filter((r) => sentFor(unlistedPath, e2)(r)).length, "宣言していない宛先に、フックから送られた").toBe(0);
}, 300_000);

inv("inv_hook_03", "フックやURL生成関数からは、他のワークスペースのデータを読めない", async () => {
  const { cms, serverAdmin, serverAdminSession } = await world();
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
  const limits = { memoryBytes: 32 * 1024 * 1024, timeMs: 1_000, asyncTimeMs: 1_000 };
  const spinMs = 20_000;
  const bombMegabytes = 256;
  const rc = await receiver();
  const host = env.harnessAllowedHost;
  const okPath = uniquePath("hook04-ok");
  const finishedPath = uniquePath("hook04-finished");
  const instance = await driver.createIsolatedInstance();
  const server: StartedServer = await driver.startServer({ ...instance.config, scriptLimits: limits });
  expect(server.outcome, `サーバーが起動しない:\n${server.output}`).toBe("ready");
  try {
    const cms = server.client!;
    const admin = instance.serverAdmin;
    const adminSession = await cms.login(admin);
    const name = templateName("hook04");
    const template = withHooks(
      name,
      {
        beforeSave: { source: runawayBeforeSave({ spinMs, bombMegabytes }), permissions: [] },
        afterPublish: {
          source: runawayAfterPublish({ spinMs, bombMegabytes, okUrl: rc.url(host, okPath), finishedUrl: rc.url(host, finishedPath) }),
          permissions: ["fetch"],
          allowedHosts: [host],
        },
      },
      { source: runawayUrlFunction({ spinMs, bombMegabytes }), permissions: [] },
    );
    expectSuccess(await cms.deployTemplateRaw(asBearer(await cms.deployToken(adminSession, admin)), template), "テンプレートのデプロイ");
    const site = await siteOn(cms, admin, adminSession, name);
    const okSlug = () => `ok-${randomUUID().slice(0, 8)}`;

    // 対照: ふつうの記事では、URL生成関数・保存前のフック・公開後のフックが動く
    const ok = await published(site, marker("hook04-ok"), okSlug());
    expect((await settledUrl(site, ok)).text.includes("/blog/ok/"), "上限の内側の関数でURLが計算されない").toBe(true);
    expect(await subtitleOf(site, ok), "上限の内側の保存前のフックが動いていない").toBe("ok");
    await rc.waitFor(sentFor(okPath, ok), 60_000);

    // --- URL生成関数 ---
    // 実行時間: 計算を続ける関数が動いている間も、配信と管理APIは応答を続ける
    const { id: spin } = await cms.createEntry(site.editorSession, site.id, { title: marker("hook04-spin"), slug: `spin-${randomUUID().slice(0, 8)}`, body: simpleBody("本文") });
    let started = Date.now();
    const spinPublish = cms.publishRaw(asSession(site.editorSession), site.id, spin);
    await sleep(0.3);
    await expectStillServing(site, ok, "上限を超えたURL生成関数の実行");
    await spinPublish; // 公開が成功するか失敗するかは問わない。応答が返ること（下の待ち時間の中で）を確かめる
    // 上限がなければ関数が実行を終えている時刻まで待ってから、関数が最後まで動いていないことを確かめる
    await sleep(Math.max(0, (spinMs + 5_000 - (Date.now() - started)) / 1000));
    expectTextNotLeaked(await urlTrail(site, spin), RUNAWAY_FINISHED, "実行時間の上限を超えたURL生成関数の結果");
    // メモリ: 大量に確保する関数は打ち切られ、サーバーは動き続ける
    const { id: bomb } = await cms.createEntry(site.editorSession, site.id, { title: marker("hook04-bomb"), slug: `bomb-${randomUUID().slice(0, 8)}`, body: simpleBody("本文") });
    await cms.publishRaw(asSession(site.editorSession), site.id, bomb);
    await sleep(10);
    expectTextNotLeaked(await urlTrail(site, bomb), RUNAWAY_FINISHED, "メモリの上限を超えたURL生成関数の結果");

    // --- フック ---
    // 保存前のフック（同期）と公開後のフック（非同期）が上限を超えている間も、配信と管理APIは応答を続ける
    const { id: hookSpin } = await cms.createEntry(site.editorSession, site.id, { title: `spin-${randomUUID().slice(0, 8)}`, slug: okSlug(), body: simpleBody("本文") });
    const { id: hookBomb } = await cms.createEntry(site.editorSession, site.id, { title: `bomb-${randomUUID().slice(0, 8)}`, slug: okSlug(), body: simpleBody("本文") });
    started = Date.now();
    const saving = saveRaw(site, hookSpin, { summary: "要約" }); // 保存の成否は問わない
    await sleep(0.3);
    await expectStillServing(site, ok, "上限を超えた保存前のフックの実行");
    await saving;
    await saveRaw(site, hookBomb, { summary: "要約" });
    // 公開後のフック（公開の成否は問わない。公開できた記事では、非同期のフックが動く）
    await cms.publishRaw(asSession(site.editorSession), site.id, hookSpin);
    await cms.publishRaw(asSession(site.editorSession), site.id, hookBomb);
    await sleep(0.3);
    await expectStillServing(site, ok, "上限を超えた公開後のフックの実行");
    await sleep(Math.max(0, (spinMs + 5_000 - (Date.now() - started)) / 1000));
    for (const [what, entryId] of [["実行時間", hookSpin], ["メモリ", hookBomb]] as const) {
      expectTextNotLeaked(String(await subtitleOf(site, entryId)), RUNAWAY_FINISHED, `${what}の上限を超えた保存前のフックの結果`);
      expect(rc.requests.filter((r) => sentFor(finishedPath, entryId)(r)).length, `${what}の上限を超えた公開後のフックが最後まで動いた`).toBe(0);
    }

    // 打ち切りの後も、サーバーは同じように動く
    const after = await published(site, marker("hook04-after"), okSlug());
    expect((await settledUrl(site, after)).text.includes("/blog/ok/"), "打ち切りの後に、ふつうの記事のURLが計算されない").toBe(true);
    expect(await subtitleOf(site, after), "打ち切りの後に、保存前のフックが動かない").toBe("ok");
    await rc.waitFor(sentFor(okPath, after), 60_000);
    expectSuccess(await cms.deliveryRaw(site.id, site.readKey.secret, site.urlQuery, { id: ok, locale: "ja" }), "打ち切りの後の配信");
  } finally {
    await server.stop();
  }
}, 420_000);

inv("inv_hook_05", "フックからの外部への通信は、例外として許可されていない内部向けのアドレスには送られず、リダイレクトをたどらない", async () => {
  const { cms, serverAdmin, serverAdminSession } = await world();
  const rc = await receiver();
  const server = await dns();
  const host = env.harnessAllowedHost;
  const blockedIp = env.harnessBlockedHost;

  // 内部向けのアドレス（例外にない）: 直接のIPアドレス、内部のアドレスに解決される名前、
  // DNSリバインディング（1回目は許可した宛先、2回目以降は内部向けのアドレスを答える）
  const blockedName = uniqueName("hook05-blocked");
  const rebindName = uniqueName("hook05-rebind");
  server.set(blockedName, [blockedIp]);
  server.set(rebindName, [env.harnessAllowedIp, blockedIp, env.harnessAllowedIp, blockedIp, blockedIp]);
  // リダイレクト: 許可した宛先が、許可した宛先の別のパスへ転送する
  const okPath = uniquePath("hook05-ok");
  const from = uniquePath("hook05-redirect");
  const to = uniquePath("hook05-final");
  const statuses = [301, 302, 307, 308];
  for (const status of statuses) {
    rc.on(`${from}-${status}`, (_req, res) => {
      res.writeHead(status, { location: rc.url(host, to) });
      res.end();
    });
  }
  const urls = [
    rc.url(host, okPath),
    rc.url(blockedIp, uniquePath("hook05-ip")),
    rc.url(blockedName, uniquePath("hook05-name")),
    rc.url(rebindName, uniquePath("hook05-rebind")),
    ...statuses.map((s) => rc.url(host, `${from}-${s}`)),
  ];
  // テンプレートで内部のアドレスを宛先として宣言しても、送信の共通の仕組みの検査は外れない
  const name = templateName("hook05");
  await deploy(
    withHooks(name, {
      afterPublish: { source: afterSending(urls), permissions: ["fetch"], allowedHosts: [host, blockedIp, blockedName, rebindName] },
    }),
  );
  const site = await siteOn(cms, serverAdmin, serverAdminSession, name);
  const entry = await published(site, marker("hook05"));

  // 対照: 許可した宛先に届き、リダイレクトを返す宛先にも送られている
  await rc.waitFor(sentFor(okPath, entry), 60_000);
  for (const s of statuses) await rc.waitFor(sentFor(`${from}-${s}`, entry), 30_000);
  await sleep(5);
  expect(rc.requests.filter((r) => r.localAddress === blockedIp).map((r) => r.path), "内部向けのアドレスに送られた").toEqual([]);
  expect(rc.requests.filter((r) => r.path.startsWith(to)).map((r) => r.path), "リダイレクト先に送られた").toEqual([]);
  expect(server.queryCount(rebindName), "リバインディングの名前が一度も問い合わされていない（DNSの設定を確認する）").toBeGreaterThan(0);
}, 180_000);
