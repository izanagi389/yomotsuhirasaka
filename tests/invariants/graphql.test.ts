// 配信API（GraphQL）の約束: INV-GQL-01〜08

import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { driver, type CmsClient } from "./support/driver.js";
import { expectNotLeaked, expectRateLimited, expectRefused, expectRejected, expectSuccess } from "./support/assert.js";
import { simpleBody } from "./support/body.js";
import { isSuccess, type HttpResponse } from "./support/http.js";
import { BASE_TEMPLATE, postType } from "./support/template.js";
import { lazyWorld, marker } from "./support/world.js";

const world = lazyWorld();

/** Cache-Control が共有キャッシュへの保存を許しているか。 */
function sharedCacheable(res: HttpResponse): boolean {
  const cc = (res.headers.get("cache-control") ?? "").toLowerCase();
  const directives = cc.split(",").map((d) => d.trim().split("=")[0]);
  if (directives.includes("private") || directives.includes("no-store")) return false;
  return directives.includes("public") || directives.includes("s-maxage");
}

async function hitUntilLimited(fn: () => Promise<HttpResponse>, max: number): Promise<boolean> {
  for (let i = 0; i < max; i++) {
    if ((await fn()).status === 429) return true;
  }
  return false;
}

inv("inv_gql_01", "本番モードの配信APIは、登録されていないクエリを拒否する", async () => {
  const { cms, a } = await world();
  // 対照: 登録済みのクエリは通る
  const ok = await cms.deliveryRaw(a.id, a.readKey.secret, a.listQuery);
  expectSuccess(ok, "登録済みのクエリ");
  expect(ok.text.includes(a.secretMarker)).toBe(true);

  // 登録済みと同じ内容でも、文のまま送ったものは拒否される
  for (const spec of [{ kind: "list" as const }, { kind: "byId" as const, include: ["body" as const] }]) {
    const res = await cms.deliveryAdHocRaw(a.id, a.readKey.secret, spec, { id: a.entryId, locale: "ja", limit: 5 });
    expectRefused(res, `登録していないクエリ（${spec.kind}）`);
    expectNotLeaked(res, a.secretMarker, `登録していないクエリ（${spec.kind}）`);
  }
  // 存在しない登録の参照
  const unknown = await cms.deliveryRaw(a.id, a.readKey.secret, { workspaceId: a.id, ref: "0".repeat(64) });
  expectRefused(unknown, "存在しないクエリの参照");
  expectNotLeaked(unknown, a.secretMarker, "存在しないクエリの参照");
});

inv("inv_gql_02", "読み取り用のAPIキーでは、クエリを登録できない", async () => {
  const { cms, a } = await world();
  expectSuccess(await cms.registerQueryRaw(a.id, a.registerKey.secret, { kind: "list", fields: ["subtitle"] }), "登録用のキーでの登録");
  for (const spec of [{ kind: "list" as const, fields: ["summary"] }, { kind: "text" as const, text: "{ __typename }" }]) {
    expectRejected(await cms.registerQueryRaw(a.id, a.readKey.secret, spec), `読み取り用のキーでの登録（${spec.kind}）`);
  }
  expectRejected(await cms.registerQueryRaw(a.id, null, { kind: "list" }), "キーなしでの登録");
});

inv("inv_gql_03", "APIキーごとの回数制限を超えたリクエストは拒否される", async () => {
  const { a } = await world();
  const server = await driver.startServer({ ...(await driver.validServerConfig()), rateLimits: { deliveryPerKey: 10 } });
  try {
    expect(server.outcome, `回数制限を小さくしたサーバーが起動しない:\n${server.output}`).toBe("ready");
    const cms: CmsClient = server.client!;
    expectSuccess(await cms.deliveryRaw(a.id, a.readKey.secret, a.listQuery), "回数制限の内側のリクエスト");
    const limited = await hitUntilLimited(() => cms.deliveryRaw(a.id, a.readKey.secret, a.listQuery), 40);
    expect(limited, "APIキーの回数制限で止められない").toBe(true);
    const res = await cms.deliveryRaw(a.id, a.readKey.secret, a.listQuery);
    expectRateLimited(res, "回数制限を超えた後のリクエスト");
    expectNotLeaked(res, a.secretMarker, "回数制限を超えた後のリクエスト");
  } finally {
    await server.stop();
  }
}, 120_000);

inv("inv_gql_04", "本番モードの配信APIでは、イントロスペクションと複数のクエリの一括送信が無効になっている", async () => {
  const { cms, a } = await world();
  const introspections = ["{ __schema { types { name fields { name } } } }", '{ __type(name: "Query") { name fields { name } } }'];
  for (const text of introspections) {
    // 登録を拒否するか、登録できても実行の結果にスキーマの情報が含まれない
    const reg = await cms.registerQueryRaw(a.id, a.registerKey.secret, { kind: "text", text });
    if (!isSuccess(reg)) continue;
    const query = await cms.registerQuery(a.id, a.registerKey.secret, { kind: "text", text });
    const res = await cms.deliveryRaw(a.id, a.readKey.secret, query);
    const leaked = isSuccess(res) && /"(fields|types)"\s*:\s*\[/.test(res.text);
    expect(leaked, `イントロスペクション（${text}）の結果にスキーマの情報が含まれている`).toBe(false);
  }
  // 一括送信
  const batch = await cms.deliveryBatchRaw(a.id, a.readKey.secret, [a.listQuery, a.byIdQuery]);
  expectRefused(batch, "複数のクエリの一括送信");
  expectNotLeaked(batch, a.secretMarker, "複数のクエリの一括送信");
});

inv("inv_gql_05", "一覧の取得件数は、変数で大きな値を指定しても、スキーマで決めた上限を超えない", async () => {
  const { cms, a } = await world();
  const limit = postType(BASE_TEMPLATE).listLimit;
  const markers: string[] = [a.secretMarker];
  for (let i = 0; i < limit + 3; i++) {
    const m = marker(`gql05-${i}`);
    markers.push(m);
    const e = await cms.createEntry(a.editorSession, a.id, { title: m, body: simpleBody(m) });
    await cms.publishEntry(a.editorSession, a.id, e.id);
  }
  for (const requested of [limit + 1, 1_000, 2_147_483_647]) {
    const res = await cms.deliveryRaw(a.id, a.readKey.secret, a.listQuery, { locale: "ja", limit: requested });
    if (!isSuccess(res)) {
      // 上限を超える値を指定したリクエスト自体を拒否するのも可
      expectRefused(res, `件数 ${requested} を指定した一覧`);
      continue;
    }
    const returned = markers.filter((m) => res.text.includes(m)).length;
    expect(returned, `件数 ${requested} を指定した一覧の件数`).toBeLessThanOrEqual(limit);
    expect(returned, `件数 ${requested} を指定した一覧が空`).toBeGreaterThan(0);
  }
});

inv("inv_gql_06", "深さや複雑さの上限を超えるクエリは登録できない", async () => {
  const { cms, a } = await world();
  // 対照: 浅いクエリは登録できる
  expectSuccess(await cms.registerQueryRaw(a.id, a.registerKey.secret, { kind: "deep", depth: 2 }), "深さ2のクエリの登録");
  expectSuccess(await cms.registerQueryRaw(a.id, a.registerKey.secret, { kind: "wide", aliases: 2 }), "別名2個のクエリの登録");
  // 上限の値は設定によるが、少なくともこの大きさは受け付けない
  expectRefused(await cms.registerQueryRaw(a.id, a.registerKey.secret, { kind: "deep", depth: 25 }), "深さ25のクエリの登録");
  expectRefused(await cms.registerQueryRaw(a.id, a.registerKey.secret, { kind: "wide", aliases: 1_000 }), "別名1000個のクエリの登録");
});

inv("inv_gql_07", "配信APIのURLのワークスペースIDと、APIキーのワークスペースが一致しないリクエストは拒否される", async () => {
  const { cms, a, b } = await world();
  expectSuccess(await cms.deliveryRaw(a.id, a.readKey.secret, a.listQuery), "一致するURLとキー");
  for (const [what, query] of [["BのURLとAのクエリ", a.listQuery], ["BのURLとBのクエリ", b.listQuery]] as const) {
    const res = await cms.deliveryRaw(b.id, a.readKey.secret, query);
    expectRejected(res, `Aのキーで、${what}`);
    expectNotLeaked(res, b.secretMarker, `Aのキーで、${what}`);
    expectNotLeaked(res, a.secretMarker, `Aのキーで、${what}`);
  }
});

inv(
  "inv_gql_08",
  "配信APIの応答は、既定で共有キャッシュに載せない指定（`Cache-Control: private`）になっている。共有キャッシュを有効にしても、公開版の読み取りだけを範囲とするキーの応答以外には共有キャッシュを許す指定が付かない",
  async () => {
    const { cms, a } = await world();

    // 既定: private
    const res = await cms.deliveryRaw(a.id, a.readKey.secret, a.listQuery);
    expectSuccess(res, "既定の設定での配信");
    expect(res.headers.get("cache-control")?.toLowerCase() ?? "", "既定の Cache-Control").toMatch(/\bprivate\b/);
    expect(sharedCacheable(res), "既定の設定で、共有キャッシュを許している").toBe(false);

    // 共有キャッシュを有効にしたサーバー
    const server = await driver.startServer({ ...(await driver.validServerConfig()), sharedCache: true });
    try {
      expect(server.outcome, `共有キャッシュを有効にしたサーバーが起動しない:\n${server.output}`).toBe("ready");
      const shared = server.client!;
      await cms.reauthenticate(a.adminSession, a.admin);
      const mixedKey = await cms.createApiKey(a.adminSession, a.id, ["read", "register"]);

      // 対照: 公開版の読み取りだけのキーには、共有キャッシュを許す指定が付く
      const readOnly = await shared.deliveryRaw(a.id, a.readKey.secret, a.listQuery);
      expectSuccess(readOnly, "共有キャッシュを有効にしたサーバーでの配信");
      expect(sharedCacheable(readOnly), "有効にしたのに、読み取りだけのキーの応答が共有キャッシュを許していない").toBe(true);
      // それ以外のキーと、拒否の応答には付かない
      const mixed = await shared.deliveryRaw(a.id, mixedKey.secret, a.listQuery);
      expect(sharedCacheable(mixed), "読み取り以外の範囲も持つキーの応答が、共有キャッシュを許している").toBe(false);
      const noKey = await shared.deliveryRaw(a.id, null, a.listQuery);
      expect(sharedCacheable(noKey), "キーのない（拒否の）応答が、共有キャッシュを許している").toBe(false);
      const unknown = await shared.deliveryRaw(a.id, a.readKey.secret, { workspaceId: a.id, ref: "0".repeat(64) });
      expect(sharedCacheable(unknown), "未登録のクエリの応答が、共有キャッシュを許している").toBe(false);
    } finally {
      await server.stop();
    }
  },
  120_000,
);
