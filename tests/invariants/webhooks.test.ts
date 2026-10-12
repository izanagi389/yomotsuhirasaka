// Webhookと外部への送信の約束: INV-WH-01〜07
//
// 受信サーバーはテストの実行環境で待ち受ける。CMSからは env.harnessAllowedHost（例外として許可済み）と
// env.harnessBlockedHost（許可していない内部のアドレス）の2つの宛先で届く。
// `*.inv.test` の名前解決は、テストの実行環境のDNSサーバー（support/dns.ts）が答える。

import { createHmac, randomUUID } from "node:crypto";
import { afterAll, expect } from "vitest";
import { inv } from "./support/inv.js";
import { asSession, driver, type CmsClient, type ParsedWebhook, type Session, type WebhookEventKind } from "./support/driver.js";
import { expectNotLeaked, expectSuccess, expectTextNotLeaked } from "./support/assert.js";
import { simpleBody } from "./support/body.js";
import { startDnsServer, type DnsServer } from "./support/dns.js";
import { env } from "./support/env.js";
import { startReceiver, type Receiver, type ReceivedRequest } from "./support/net.js";
import { operationsOf } from "./support/operations.js";
import { startRecordingProxy } from "./support/proxy.js";
import { lazyWorld, marker, sleep } from "./support/world.js";

const world = lazyWorld();

let receiverPromise: Promise<Receiver> | null = null;
let dnsPromise: Promise<DnsServer> | null = null;
const receiver = () => (receiverPromise ??= startReceiver());
const dns = () => (dnsPromise ??= startDnsServer(env.harnessDnsPort));
afterAll(async () => {
  if (receiverPromise) await (await receiverPromise).close();
  if (dnsPromise) await (await dnsPromise).close();
});

const uniqueName = (label: string) => `${label}-${randomUUID().slice(0, 8)}.inv.test`;
const uniquePath = (label: string) => `/${label}-${randomUUID().slice(0, 8)}`;

function parseOrNull(body: string): ParsedWebhook | null {
  try {
    return driver.webhook.parse(body);
  } catch {
    return null;
  }
}

/** 記事を作って公開し、そのIDを返す。 */
async function publishNew(cms: CmsClient, session: Session, workspaceId: string, mark = marker("wh")): Promise<string> {
  const e = await cms.createEntry(session, workspaceId, { title: mark, body: simpleBody(mark) });
  await cms.publishEntry(session, workspaceId, e.id);
  return e.id;
}

const forEntry = (path: string, entryId: string) => (r: ReceivedRequest) => r.path.startsWith(path) && parseOrNull(r.body)?.entryId === entryId;

inv(
  "inv_wh_01",
  "名前解決の結果が内部向けのアドレスになる送信先には、例外として許可されていない限りWebhookを送らない。検査の後に名前解決の結果が変わっても、検査したアドレス以外には接続しない",
  async () => {
    const { cms, a } = await world();
    const rc = await receiver();
    const server = await dns();
    const blockedIp = env.harnessBlockedHost;
    const toBlocked = (r: ReceivedRequest) => r.localAddress === blockedIp;
    await cms.reauthenticate(a.adminSession, a.admin);

    // 対照: 例外として許可した宛先には届く
    const okPath = uniquePath("wh01-ok");
    await cms.createWebhook(a.adminSession, a.id, rc.url(env.harnessAllowedHost, okPath));
    const okEntry = await publishNew(cms, a.editorSession, a.id);
    await rc.waitFor(forEntry(okPath, okEntry));

    // 内部向けのアドレス（例外にない）: 直接のIPアドレス、内部のアドレスに解決される名前
    const blockedName = uniqueName("wh01-blocked");
    server.set(blockedName, [blockedIp]);
    const blockedUrls = [rc.url(blockedIp, uniquePath("wh01-ip")), rc.url(blockedName, uniquePath("wh01-name"))];
    for (const url of blockedUrls) {
      try {
        await cms.createWebhook(a.adminSession, a.id, url);
      } catch {
        // 登録の時点で拒否するのも可
      }
    }
    // DNSリバインディング: 1回目は許可した宛先、2回目以降は内部向けのアドレスを答える
    const rebinding = uniqueName("wh01-rebind");
    server.set(rebinding, [env.harnessAllowedIp, blockedIp, env.harnessAllowedIp, blockedIp, blockedIp]);
    try {
      await cms.createWebhook(a.adminSession, a.id, rc.url(rebinding, uniquePath("wh01-rebind")));
    } catch {
      // 登録の時点で拒否するのも可
    }

    for (let i = 0; i < 3; i++) await publishNew(cms, a.editorSession, a.id);
    const leaked = await rc.collect(toBlocked, 10_000);
    expect(leaked.map((r) => r.path), "内部向けのアドレスに送られた").toEqual([]);
    expect(server.queryCount(rebinding), "リバインディングの名前が一度も問い合わされていない（DNSの設定を確認する）").toBeGreaterThan(0);
  },
  120_000,
);

inv("inv_wh_02", "Webhookの送信はリダイレクトをたどらない", async () => {
  const { cms, a } = await world();
  const rc = await receiver();
  const from = uniquePath("wh02-redirect");
  const to = uniquePath("wh02-final");
  for (const status of [301, 302, 307, 308]) {
    rc.on(`${from}-${status}`, (_req, res) => {
      res.writeHead(status, { location: rc.url(env.harnessAllowedHost, to) });
      res.end();
    });
  }
  await cms.reauthenticate(a.adminSession, a.admin);
  for (const status of [301, 302, 307, 308]) {
    await cms.createWebhook(a.adminSession, a.id, rc.url(env.harnessAllowedHost, `${from}-${status}`));
  }
  const entry = await publishNew(cms, a.editorSession, a.id);
  for (const status of [301, 302, 307, 308]) await rc.waitFor(forEntry(`${from}-${status}`, entry));
  const followed = await rc.collect((r) => r.path.startsWith(to), 5_000);
  expect(followed.length, "リダイレクト先に送られた").toBe(0);
});

inv("inv_wh_03", "Webhookの署名には、送信時刻とイベントIDが含まれる", async () => {
  const { cms, a } = await world();
  const rc = await receiver();
  const path = uniquePath("wh03");
  await cms.reauthenticate(a.adminSession, a.admin);
  const hook = await cms.createWebhook(a.adminSession, a.id, rc.url(env.harnessAllowedHost, path));

  const e1 = await publishNew(cms, a.editorSession, a.id);
  const e2 = await publishNew(cms, a.editorSession, a.id);
  const ids: string[] = [];
  for (const entry of [e1, e2]) {
    const req = await rc.waitFor(forEntry(path, entry));
    const s = driver.webhook.signing(req.headers, req.body);
    expect(Math.abs(s.timestamp - Date.now() / 1000), "署名の送信時刻が現在とずれている").toBeLessThan(300);
    expect(s.eventId.length, "イベントIDが空").toBeGreaterThan(0);
    expect(s.signingInput.includes(String(s.timestamp)), "署名の対象に送信時刻が含まれていない").toBe(true);
    expect(s.signingInput.includes(s.eventId), "署名の対象にイベントIDが含まれていない").toBe(true);
    expect(s.signingInput.includes(req.body), "署名の対象に本文が含まれていない").toBe(true);
    const expected = createHmac("sha256", s.keyBytes(hook.secret)).update(s.signingInput).digest(s.encoding);
    expect(s.signatures, "署名が、送信時刻・イベントID・本文と秘密から作った値と一致しない").toContain(expected);
    // 送信時刻を変えると、署名が一致しなくなる（受け手が古いものを拒否できる）
    const tampered = s.signingInput.replace(String(s.timestamp), String(s.timestamp - 3600));
    expect(s.signatures).not.toContain(createHmac("sha256", s.keyBytes(hook.secret)).update(tampered).digest(s.encoding));
    ids.push(s.eventId);
  }
  expect(new Set(ids).size, "別のイベントに同じイベントIDが付いている").toBe(2);
});

inv("inv_wh_04", "Webhookのテスト送信などの管理APIの応答には、送信先が返した本文が含まれない", async () => {
  const w = await world();
  const { cms, a } = w;
  const rc = await receiver();
  const responseMarker = marker("wh04-response");
  const okPath = uniquePath("wh04-ok");
  const errorPath = uniquePath("wh04-error");
  rc.on(okPath, (_req, res) => {
    res.writeHead(200, { "content-type": "text/plain", "x-echo": responseMarker });
    res.end(`送信先の応答 ${responseMarker}`);
  });
  rc.on(errorPath, (_req, res) => {
    res.writeHead(500, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: responseMarker }));
  });

  await cms.reauthenticate(a.adminSession, a.admin);
  for (const path of [okPath, errorPath]) {
    const hook = await cms.createWebhook(a.adminSession, a.id, rc.url(env.harnessAllowedHost, path));
    const res = await cms.testWebhookRaw(a.adminSession, a.id, hook.id);
    await rc.waitFor((r) => r.path.startsWith(path));
    expectNotLeaked(res, responseMarker, `テスト送信（${path}）の応答`);
  }
  // 送信の記録などの、他の読み取りの操作にも出ない
  await publishNew(cms, a.editorSession, a.id);
  await rc.waitFor((r) => r.path.startsWith(okPath) && parseOrNull(r.body) !== null);
  const target = { workspaceId: a.id, entryId: a.entryId, userId: a.editor.userId, apiKeyId: a.readKey.id };
  for (const op of operationsOf(cms).filter((o) => !o.mutating && o.scope === "workspace")) {
    expectNotLeaked(await op.call(asSession(a.adminSession), target), responseMarker, `${op.name} の応答`);
  }
});

inv(
  "inv_wh_05",
  "出口のプロキシを設定した場合も、既定では中継の依頼の宛先は検査済みのIPアドレスであり、内部向けのアドレスへの送信は依頼されない",
  async () => {
    // 送信のジョブを他のサーバーに取られないよう、新しいインスタンスで動かす
    const proxy = await startRecordingProxy();
    const tlsReceiver = await startReceiver({ tls: env.harnessTls });
    const server = await dns();
    const instance = await driver.createIsolatedInstance();
    const started = await driver.startServer({ ...instance.config, egressProxy: { url: proxy.url(env.harnessAllowedHost) } });
    try {
      expect(started.outcome, `出口のプロキシを設定したサーバーが起動しない:\n${started.output}`).toBe("ready");
      const cms = started.client!;
      const sa = await cms.login(instance.serverAdmin);
      await cms.reauthenticate(sa, instance.serverAdmin);
      const ws = (await cms.createWorkspace(sa, "inv-wh-05")).id;
      const admin = await cms.createMember(sa, ws, "admin");
      const adminSession = await cms.login(admin);
      await cms.reauthenticate(adminSession, admin);

      const okName = uniqueName("wh05-ok");
      const blockedName = uniqueName("wh05-blocked");
      const rebindName = uniqueName("wh05-rebind");
      server.set(okName, [env.harnessAllowedIp]);
      server.set(blockedName, [env.harnessBlockedHost]);
      server.set(rebindName, [env.harnessAllowedIp, env.harnessBlockedHost, env.harnessBlockedHost]);
      const okPath = uniquePath("wh05-ok");
      await cms.createWebhook(adminSession, ws, tlsReceiver.url(okName, okPath));
      for (const name of [blockedName, rebindName]) {
        try {
          await cms.createWebhook(adminSession, ws, tlsReceiver.url(name, uniquePath("wh05-ng")));
        } catch {
          // 登録の時点で拒否するのも可
        }
      }
      const entry = await publishNew(cms, adminSession, ws);

      // 対照: プロキシ経由で、許可した宛先に届く
      await tlsReceiver.waitFor(forEntry(okPath, entry), 30_000);
      await sleep(5);

      const connects = proxy.requests.filter((r) => r.method === "CONNECT").map((r) => r.target);
      expect(connects.length, "プロキシに中継の依頼が届いていない").toBeGreaterThan(0);
      for (const target of connects) {
        const host = target.slice(0, target.lastIndexOf(":")).replace(/^\[|\]$/g, "");
        expect(/^[0-9.]+$|:/.test(host), `中継の依頼の宛先がIPアドレスではない: ${target}`).toBe(true);
        expect(host, "内部向けのアドレスへの中継が依頼された").not.toBe(env.harnessBlockedHost);
      }
      expect(connects, "許可した宛先への中継が、検査したIPアドレスで依頼されていない").toContain(`${env.harnessAllowedIp}:${tlsReceiver.port}`);
      expect(tlsReceiver.requests.filter((r) => r.localAddress === env.harnessBlockedHost), "内部向けのアドレスに届いた").toEqual([]);
    } finally {
      await started.stop();
      await tlsReceiver.close();
      await proxy.close();
    }
  },
  300_000,
);

inv(
  "inv_wh_06",
  "記事の公開・更新・非公開化・削除・訂正としての公開・版の本文の抹消・公開設定の変更・URLの変更のたびに、共有キャッシュの有無にかかわらず、対応するイベントのWebhookが送られる",
  async () => {
    const { cms, a } = await world();
    const rc = await receiver();
    const path = uniquePath("wh06");
    await cms.reauthenticate(a.adminSession, a.admin);
    await cms.createWebhook(a.adminSession, a.id, rc.url(env.harnessAllowedHost, path));
    const types = driver.webhook.eventTypes();
    const s = a.editorSession;
    const expectEvent = async (entryId: string, kind: WebhookEventKind) => {
      await rc.waitFor((r) => r.path.startsWith(path) && parseOrNull(r.body)?.entryId === entryId && parseOrNull(r.body)?.type === types[kind], 30_000);
    };

    // 共有キャッシュは無効（既定）のまま確かめる
    const firstMarker = marker("wh06-v1");
    const e = await cms.createEntry(s, a.id, { title: "イベント", body: simpleBody(firstMarker), slug: `wh06-${randomUUID().slice(0, 8)}` });
    await cms.publishEntry(s, a.id, e.id);
    await expectEvent(e.id, "published");

    await cms.saveDraft(s, a.id, e.id, { body: simpleBody(marker("wh06-v2")) });
    await cms.publishEntry(s, a.id, e.id);
    await expectEvent(e.id, "updated");

    await cms.saveDraft(s, a.id, e.id, { slug: `wh06-${randomUUID().slice(0, 8)}` });
    await cms.publishEntry(s, a.id, e.id);
    await expectEvent(e.id, "urlChanged");

    await cms.changePublishSettings(s, a.id, e.id);
    await expectEvent(e.id, "settingsChanged");

    await cms.saveDraft(s, a.id, e.id, { body: simpleBody(marker("wh06-v4")) });
    await cms.publishEntry(s, a.id, e.id, { correction: true });
    await expectEvent(e.id, "corrected");

    const versions = await cms.listVersions(a.adminSession, a.id, e.id);
    let first: string | null = null;
    for (const v of versions) {
      if ((await cms.getVersionRaw(asSession(a.adminSession), a.id, e.id, v.id)).text.includes(firstMarker)) first = v.id;
    }
    await cms.reauthenticate(a.adminSession, a.admin);
    expectSuccess(await cms.eraseVersionRaw(a.adminSession, a.id, e.id, first!, []), "最初の版の抹消");
    await expectEvent(e.id, "erased");

    await cms.unpublishEntry(s, a.id, e.id);
    await expectEvent(e.id, "unpublished");

    await cms.deleteEntry(a.adminSession, a.id, e.id);
    await expectEvent(e.id, "deleted");
  },
  180_000,
);

inv("inv_wh_07", "Webhookの本文には、記事の本文と下書きの内容が含まれない", async () => {
  const { cms, a } = await world();
  const rc = await receiver();
  const path = uniquePath("wh07");
  await cms.reauthenticate(a.adminSession, a.admin);
  await cms.createWebhook(a.adminSession, a.id, rc.url(env.harnessAllowedHost, path));
  const s = a.editorSession;

  const bodyMarker = marker("wh07-body");
  const draftMarker = marker("wh07-draft");
  const e = await cms.createEntry(s, a.id, { title: "Webhookの本文", body: simpleBody(bodyMarker) });
  await cms.publishEntry(s, a.id, e.id);
  await cms.saveDraft(s, a.id, e.id, { title: draftMarker, body: simpleBody(draftMarker) }); // 公開しない下書き
  await cms.changePublishSettings(s, a.id, e.id);
  await cms.unpublishEntry(s, a.id, e.id);

  const received = await rc.collect((r) => r.path.startsWith(path) && parseOrNull(r.body)?.entryId === e.id, 10_000);
  expect(received.length, "この記事のWebhookが届いていない").toBeGreaterThanOrEqual(3);
  for (const r of received) {
    const all = `${r.body}\n${JSON.stringify(r.headers)}`;
    expectTextNotLeaked(all, bodyMarker, "Webhook（記事の本文）");
    expectTextNotLeaked(all, draftMarker, "Webhook（下書きの内容）");
  }
});
