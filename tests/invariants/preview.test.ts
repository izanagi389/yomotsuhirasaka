// プレビューの約束: INV-PRV-01, INV-PRV-02, INV-PRV-03
//
// プレビュー用トークンは、配信APIに読み取り用のAPIキーと一緒に付けると、対象の記事だけを下書きで返す
// （1件を取るクエリ（IDとスラッグ）だけ。一覧には出さない。03-content-and-editing.md「プレビュー（Q72）」）。
// トークンだけでは配信APIを使えない。失効（手動・有効期間の経過）した後は、下書きを読めない。
// ライブプレビューのメッセージの受け手は、設定したオリジンの、期待した窓からのメッセージだけを受け付ける（13-security.md）。
// 漏れてはいけない下書きの内容は、一意な目印で見分ける。

import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { driver, type RegisteredQuery } from "./support/driver.js";
import { expectRejected, expectSuccess } from "./support/assert.js";
import { simpleBody } from "./support/body.js";
import { lazyWorld, marker, sleep } from "./support/world.js";

const world = lazyWorld();

/** 公開した後に、下書きだけを変えた記事。 */
async function publishedWithDraft(label: string) {
  const { cms, a } = await world();
  const published = marker(`${label}-published`);
  const draft = marker(`${label}-draft`);
  const e = await cms.createEntry(a.editorSession, a.id, { title: published, body: simpleBody(published) });
  await cms.publishEntry(a.editorSession, a.id, e.id);
  await cms.saveDraft(a.editorSession, a.id, e.id, { title: draft, body: simpleBody(draft) });
  const slug = (await cms.getDraft(a.editorSession, a.id, e.id)).slug;
  return { id: e.id, slug, published, draft };
}

inv("inv_prv_01", "プレビュー用トークンは対象の記事の下書きだけを読め、失効後は使えない", async () => {
  const { cms, a } = await world();
  const bySlug: RegisteredQuery = await cms.registerQuery(a.id, a.registerKey.secret, { kind: "bySlug", include: ["body"] });
  const list: RegisteredQuery = await cms.registerQuery(a.id, a.registerKey.secret, { kind: "list", fields: ["title"] });
  const x = await publishedWithDraft("prv01-x");
  const y = await publishedWithDraft("prv01-y");
  const neverPublished = marker("prv01-new");
  const z = await cms.createEntry(a.editorSession, a.id, { title: neverPublished, body: simpleBody(neverPublished) });

  const tx = await cms.issuePreviewToken(a.editorSession, a.id, x.id);
  const preview = (token: string, query: RegisteredQuery, variables: Record<string, unknown>) =>
    cms.deliveryPreviewRaw(a.id, a.readKey.secret, token, query, variables);

  // 対象の記事は、IDでもスラッグでも下書きで返る
  const xById = await preview(tx.token, a.byIdQuery, { id: x.id });
  expectSuccess(xById, "トークン付きの、対象の記事の取得（ID）");
  expect(xById.text.includes(x.draft), "対象の記事の下書きが返らない（ID）").toBe(true);
  const xBySlug = await preview(tx.token, bySlug, { slug: x.slug });
  expectSuccess(xBySlug, "トークン付きの、対象の記事の取得（スラッグ）");
  expect(xBySlug.text.includes(x.draft), "対象の記事の下書きが返らない（スラッグ）").toBe(true);

  // 他の記事は公開中の内容のまま。一覧にも下書きは出ない
  const yById = await preview(tx.token, a.byIdQuery, { id: y.id });
  expectSuccess(yById, "トークン付きの、他の記事の取得");
  expect(yById.text.includes(y.draft), "対象でない記事の下書きが返った").toBe(false);
  expect(yById.text.includes(y.published), "対象でない記事の公開中の内容が返らない").toBe(true);
  const listed = await preview(tx.token, list, {});
  expectSuccess(listed, "トークン付きの一覧");
  expect(listed.text.includes(x.draft), "一覧に対象の記事の下書きが出た").toBe(false);

  // まだ一度も公開していない記事も、その記事のトークンなら1件のクエリで読める（対照）。他の記事のトークンでは読めない
  const tz = await cms.issuePreviewToken(a.editorSession, a.id, z.id);
  expect((await preview(tz.token, a.byIdQuery, { id: z.id })).text.includes(neverPublished), "未公開の記事の下書きが、その記事のトークンで返らない").toBe(true);
  expect((await preview(tx.token, a.byIdQuery, { id: z.id })).text.includes(neverPublished), "他の記事のトークンで、未公開の記事の下書きが返った").toBe(false);

  // 失効させた後は、下書きを読めない
  await cms.revokePreviewToken(a.editorSession, a.id, x.id, tx.id);
  expect((await preview(tx.token, a.byIdQuery, { id: x.id })).text.includes(x.draft), "失効させたトークンで下書きが返った（ID）").toBe(false);
  expect((await preview(tx.token, bySlug, { slug: x.slug })).text.includes(x.draft), "失効させたトークンで下書きが返った（スラッグ）").toBe(false);

  // 有効期間が過ぎた後も、下書きを読めない
  const short = await cms.issuePreviewToken(a.editorSession, a.id, x.id, { ttlSeconds: 1 });
  expect((await preview(short.token, a.byIdQuery, { id: x.id })).text.includes(x.draft), "有効期間内のトークンで下書きが返らない（対照）").toBe(true);
  await sleep(2.5);
  expect((await preview(short.token, a.byIdQuery, { id: x.id })).text.includes(x.draft), "有効期間を過ぎたトークンで下書きが返った").toBe(false);
});

inv("inv_prv_02", "プレビュー用トークンは、配信APIのAPIキーとして使えない", async () => {
  const { cms, a } = await world();
  const x = await publishedWithDraft("prv02");
  const { token } = await cms.issuePreviewToken(a.editorSession, a.id, x.id);

  // 対照: 読み取り用のキーなら、同じクエリが通る
  expectSuccess(await cms.deliveryRaw(a.id, a.readKey.secret, a.byIdQuery, { id: x.id }), "読み取り用のキーでの取得");

  // APIキーの場所にトークンを入れても、プレビューのヘッダーだけを付けても、使えない
  const asKey = await cms.deliveryRaw(a.id, token, a.byIdQuery, { id: x.id });
  expectRejected(asKey, "トークンをAPIキーとして使った取得");
  expect(asKey.text.includes(x.draft) || asKey.text.includes(x.published), "トークンをAPIキーとして使った応答に、記事の内容がある").toBe(false);
  const tokenOnly = await cms.deliveryPreviewRaw(a.id, null, token, a.byIdQuery, { id: x.id });
  expectRejected(tokenOnly, "APIキーなしで、プレビューのヘッダーだけを付けた取得");
  expect(tokenOnly.text.includes(x.draft) || tokenOnly.text.includes(x.published), "APIキーなしの応答に、記事の内容がある").toBe(false);
});

inv("inv_prv_03", "ライブプレビューは、設定されたサイト以外のドメインからのメッセージを受け付けない", async () => {
  const site = "https://site.inv.test";
  const accepts = (origin: string, fromExpectedWindow = true) => driver.livePreview.accepts({ origin, fromExpectedWindow }, [site]);

  // 対照: 設定したサイトの、期待した窓からのメッセージは受け付ける
  expect(await accepts(site), "設定したサイトからのメッセージを受け付けない").toBe(true);
  // 設定していないドメイン・似たドメイン・スキームやポートの違うオリジンは受け付けない
  for (const other of ["https://evil.inv.test", "https://site.inv.test.evil.test", "https://sub.site.inv.test", "http://site.inv.test", "https://site.inv.test:8443", "null"]) {
    expect(await accepts(other), `設定していないオリジン ${other} からのメッセージを受け付けた`).toBe(false);
  }
  // オリジンが正しくても、期待した窓（プレビューの iframe）以外からのメッセージは受け付けない
  expect(await accepts(site, false), "期待した窓以外からのメッセージを受け付けた").toBe(false);
});
