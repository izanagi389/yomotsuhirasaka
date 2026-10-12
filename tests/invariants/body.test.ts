// 本文の約束: INV-ANC-01, 02, INV-VAL-01〜04

import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asSession, driver, type EntryInput } from "./support/driver.js";
import { expectRefused, expectSuccess } from "./support/assert.js";
import { doc, heading, headingText, headingsOf, link, paragraph, text, type TiptapDoc, type TiptapNode } from "./support/body.js";
import { lazyWorld, marker } from "./support/world.js";

const world = lazyWorld();

const anchorsByText = (d: TiptapDoc): Map<string, string | undefined> =>
  new Map(headingsOf(d).map((h) => [headingText(h), driver.body.anchorIdOf(h)]));

/** 不正な本文が、作成でも既存の記事の保存でも拒否され、下書きが変わらないことを確かめる。 */
async function expectBodyRefused(body: TiptapDoc, what: string): Promise<void> {
  const { cms, a } = await world();
  const s = a.editorSession;
  expectRefused(await cms.createEntryRaw(asSession(s), a.id, { title: marker("val"), body }), `${what}を含む記事の作成`);

  const goodMarker = marker("val-good");
  const e = await cms.createEntry(s, a.id, { title: "検証", body: doc(paragraph(goodMarker)) });
  const before = await cms.getDraft(s, a.id, e.id);
  expectRefused(await cms.saveDraftRaw(asSession(s), a.id, e.id, { body }, before.revision), `${what}を含む下書きの保存`);
  const after = await cms.getDraft(s, a.id, e.id);
  expect(after.revision, `${what}の保存が拒否された後に、リビジョン番号が変わった`).toBe(before.revision);
  expect(JSON.stringify(after.body).includes(goodMarker), `${what}の保存が拒否された後に、下書きが変わった`).toBe(true);
}

async function expectBodyAccepted(body: TiptapDoc, what: string): Promise<void> {
  const { cms, a } = await world();
  const input: EntryInput = { title: marker("val-ok"), body };
  expectSuccess(await cms.createEntryRaw(asSession(a.editorSession), a.id, input), `${what}を含む記事の作成`);
}

inv("inv_anc_01", "見出しに付与されたアンカーIDは、見出しの文字列の変更、見出しの移動、記事の再保存・再公開で変わらない", async () => {
  const { cms, a } = await world();
  const s = a.editorSession;
  const e = await cms.createEntry(s, a.id, {
    title: "アンカー",
    body: doc(heading("はじめに"), paragraph("本文1"), heading("使い方"), paragraph("本文2"), heading("まとめ", 3)),
  });
  const original = anchorsByText((await cms.getDraft(s, a.id, e.id)).body);
  const intro = original.get("はじめに");
  const usage = original.get("使い方");
  const summary = original.get("まとめ");
  for (const id of [intro, usage, summary]) expect(id, "見出しにアンカーIDが付与されていない").toBeTruthy();
  expect(new Set([intro, usage, summary]).size, "アンカーIDが重複している").toBe(3);

  // 文字列の変更と移動: 保存されている見出しのノード（アンカーIDを含む）をそのまま使って並べ替える
  const saved = (await cms.getDraft(s, a.id, e.id)).body;
  const [h1, p1, h2, p2, h3] = saved.content as [TiptapNode, TiptapNode, TiptapNode, TiptapNode, TiptapNode];
  const renamed: TiptapNode = { ...h1, content: [text("はじめに（改訂）")] };
  await cms.saveDraft(s, a.id, e.id, { body: doc(h2, p2, renamed, p1, h3) });
  const moved = anchorsByText((await cms.getDraft(s, a.id, e.id)).body);
  expect(moved.get("はじめに（改訂）"), "文字列を変えた見出しのアンカーID").toBe(intro);
  expect(moved.get("使い方"), "移動した見出しのアンカーID").toBe(usage);
  expect(moved.get("まとめ"), "そのままの見出しのアンカーID").toBe(summary);

  // 公開、再保存、再公開
  await cms.publishEntry(s, a.id, e.id);
  const current = await cms.getDraft(s, a.id, e.id);
  await cms.saveDraft(s, a.id, e.id, { body: current.body });
  await cms.publishEntry(s, a.id, e.id);
  const republished = anchorsByText((await cms.getDraft(s, a.id, e.id)).body);
  expect(republished.get("はじめに（改訂）")).toBe(intro);
  expect(republished.get("使い方")).toBe(usage);
  expect(republished.get("まとめ")).toBe(summary);

  // 配信される本文にも同じアンカーIDが載っている
  const deliveredText = (await cms.deliveryRaw(a.id, a.readKey.secret, a.byIdQuery, { id: e.id, locale: "ja" })).text;
  for (const id of [intro, usage, summary]) expect(deliveredText.includes(id!), `配信された本文にアンカーID ${id} がない`).toBe(true);
});

inv(
  "inv_anc_02",
  "新しく追加された見出しには、同じ記事の中で重複しないアンカーIDが付与される。重複したアンカーIDを含む本文を保存すると、2つ目以降の見出しに新しいアンカーIDが付与される",
  async () => {
    const { cms, a } = await world();
    const s = a.editorSession;
    const e = await cms.createEntry(s, a.id, { title: "アンカー2", body: doc(heading("既存"), paragraph("x")) });
    const existing = anchorsByText((await cms.getDraft(s, a.id, e.id)).body).get("既存");

    // 新しい見出しを足す（IDなし）
    const saved = (await cms.getDraft(s, a.id, e.id)).body;
    await cms.saveDraft(s, a.id, e.id, { body: doc(...saved.content, heading("追加1"), heading("追加2")) });
    const added = anchorsByText((await cms.getDraft(s, a.id, e.id)).body);
    expect(added.get("既存")).toBe(existing);
    const ids = [added.get("既存"), added.get("追加1"), added.get("追加2")];
    for (const id of ids) expect(id, "見出しにアンカーIDがない").toBeTruthy();
    expect(new Set(ids).size, "追加した見出しのアンカーIDが重複している").toBe(3);

    // 重複したアンカーIDを含む本文（コピー・取り込み・APIからの書き込みを想定）
    const dup = "inv-dup-anchor";
    const withDup = doc(
      driver.body.withAnchor(heading("1つ目"), dup),
      paragraph("a"),
      driver.body.withAnchor(heading("2つ目"), dup),
      driver.body.withAnchor(heading("3つ目"), dup),
      driver.body.withAnchor(heading("既存のIDと重複"), existing!),
    );
    const e2 = await cms.createEntry(s, a.id, { title: "重複", body: withDup });
    const result = anchorsByText((await cms.getDraft(s, a.id, e2.id)).body);
    expect(result.get("1つ目"), "1つ目の見出しのアンカーIDが変えられた").toBe(dup);
    const all = [...result.values()];
    for (const id of all) expect(id, "見出しにアンカーIDがない").toBeTruthy();
    expect(new Set(all).size, "保存した後もアンカーIDが重複している").toBe(all.length);

    // 既存の記事の保存でも同じ
    const cur = await cms.getDraft(s, a.id, e.id);
    await cms.saveDraft(s, a.id, e.id, { body: doc(...cur.body.content, driver.body.withAnchor(heading("後から重複"), existing!)) });
    const after = anchorsByText((await cms.getDraft(s, a.id, e.id)).body);
    expect(after.get("既存"), "元からある見出しのアンカーIDが変えられた").toBe(existing);
    expect(after.get("後から重複"), "後から足した重複の見出しに新しいアンカーIDが付いていない").not.toBe(existing);
    expect(new Set(after.values()).size).toBe(after.size);
  },
);

inv("inv_val_01", "マニフェストに定義されていないノードや属性を含む本文は、保存時に拒否される", async () => {
  await expectBodyAccepted(doc(paragraph("通常の段落", link("リンク", "https://example.com"))), "定義されたノード");
  await expectBodyRefused(doc({ type: "script", content: [text("alert(1)")] }), "定義されていないノード");
  await expectBodyRefused(doc({ type: "rawHtml", attrs: { html: "<img src=x onerror=alert(1)>" } }), "定義されていないノード（生のHTML）");
  await expectBodyRefused(doc({ type: "paragraph", attrs: { onclick: "alert(1)" }, content: [text("x")] }), "定義されていない属性");
  await expectBodyRefused(doc({ type: "heading", attrs: { level: 2, style: "color:red" }, content: [text("x")] }), "見出しの定義されていない属性");
  await expectBodyRefused(doc(paragraph(text("x", [{ type: "strike" }]))), "定義されていないマーク");
  await expectBodyRefused(doc(paragraph(text("x", [{ type: "link", attrs: { href: "https://example.com", onmouseover: "alert(1)" } }]))), "マークの定義されていない属性");
});

inv("inv_val_02", "許可していないスキーム（`javascript:` など）のリンクを含む本文は、保存時に拒否される", async () => {
  for (const href of ["https://example.com/a", "http://example.com/a", "mailto:a@example.com", "tel:+81-3-0000-0000", "/relative/path", "#section"]) {
    await expectBodyAccepted(doc(paragraph(link("ok", href))), `リンク ${href}`);
  }
  for (const href of [
    "javascript:alert(1)",
    "JaVaScRiPt:alert(1)",
    " javascript:alert(1)",
    "java\tscript:alert(1)",
    "javascript&colon;alert(1)",
    "data:text/html,<script>alert(1)</script>",
    "vbscript:msgbox(1)",
    "file:///etc/passwd",
  ]) {
    await expectBodyRefused(doc(paragraph(link("ng", href))), `リンク ${JSON.stringify(href)}`);
  }
});

inv("inv_val_03", "許可していない提供元の埋め込みを含む本文は、保存時に拒否される", async () => {
  await expectBodyAccepted(doc(driver.body.embed("https://www.youtube.com/embed/aaaaaaaaaaa")), "許可した提供元の埋め込み");
  for (const url of [
    "https://evil.inv.test/embed/x",
    "https://www.youtube.com.evil.inv.test/embed/x",
    "https://evil.inv.test/?u=https://www.youtube.com/embed/x",
    "https://www.youtube.com@evil.inv.test/embed/x",
    "javascript:alert(1)",
  ]) {
    await expectBodyRefused(doc(driver.body.embed(url)), `埋め込み ${url}`);
  }
});

inv("inv_val_04", "マニフェストで許可されていない子ノードを含む本文は、保存時に拒否される", async () => {
  await expectBodyAccepted(
    doc({ type: "bulletList", content: [{ type: "listItem", content: [paragraph("項目")] }] }),
    "正しい入れ子のリスト",
  );
  await expectBodyRefused(doc(text("文書の直下の文字")), "文書の直下の文字");
  await expectBodyRefused(doc({ type: "paragraph", content: [paragraph("段落の中の段落")] }), "段落の中の段落");
  await expectBodyRefused(doc({ type: "heading", attrs: { level: 2 }, content: [{ type: "bulletList", content: [] }] }), "見出しの中のリスト");
  await expectBodyRefused(doc({ type: "bulletList", content: [paragraph("リストの直下の段落")] }), "リストの直下の段落");
});
