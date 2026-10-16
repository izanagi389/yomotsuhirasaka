// 変換器とカスタムノードの約束: INV-NODE-01, INV-CNV-01
//
// - カスタムノードはテンプレートに宣言的に書き、変換関数（Markdown必須、HTML任意）を付ける。
//   デプロイの拒否は、CLIを通さずデプロイのAPIを直接呼んで確かめる（13-security.md「デプロイ」）。
// - 変換器のHTMLは、配信APIの本文のHTML（bodyHtml）で確かめる。HTMLはパーサーを使わず、出てはいけないものを
//   保守的に探す（support/html.ts）。
// テンプレートのデプロイはテンプレートを使うすべてのワークスペースに効くため、テストごとに別の名前のテンプレートを使う。

import { randomUUID } from "node:crypto";
import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asBearer, asSession, driver, type IssuedSecret, type Session } from "./support/driver.js";
import { expectRefused, expectSuccess } from "./support/assert.js";
import type { HttpResponse } from "./support/http.js";
import { doc, heading, link, paragraph, text, type TiptapNode } from "./support/body.js";
import { expectNoActiveHtml, findString, tagsOf } from "./support/html.js";
import { pngImage } from "./support/images.js";
import { customToHtml, customToMarkdown } from "./support/scripts.js";
import { cloneTemplate, type CustomNodeSpec, type TemplateSpec } from "./support/template.js";
import { lazyWorld, marker } from "./support/world.js";

const world = lazyWorld();
const templateName = (label: string) => `inv-${label}-${randomUUID().slice(0, 8)}`;

async function deployRaw(t: TemplateSpec): Promise<HttpResponse> {
  const { cms, serverAdmin, serverAdminSession } = await world();
  return cms.deployTemplateRaw(asBearer(await cms.deployToken(serverAdminSession, serverAdmin)), t);
}

const withNodes = (name: string, nodes: CustomNodeSpec[], edit?: (t: TemplateSpec) => void): TemplateSpec =>
  cloneTemplate(name, (t) => {
    t.customNodes = nodes;
    edit?.(t);
  });

interface Site {
  id: string;
  editorSession: Session;
  adminSession: Session;
  readKey: IssuedSecret;
  registerKey: IssuedSecret;
}

async function siteOn(template: string): Promise<Site> {
  const { cms, serverAdmin, serverAdminSession } = await world();
  await cms.reauthenticate(serverAdminSession, serverAdmin);
  const { id } = await cms.createWorkspace(serverAdminSession, `${template}-ws-${randomUUID().slice(0, 6)}`, { template });
  const admin = await cms.createMember(serverAdminSession, id, "admin");
  const adminSession = await cms.login(admin);
  const editor = await cms.createMember(adminSession, id, "editor");
  const editorSession = await cms.login(editor);
  await cms.reauthenticate(adminSession, admin);
  const readKey = await cms.createApiKey(adminSession, id, ["read"]);
  const registerKey = await cms.createApiKey(adminSession, id, ["register"]);
  return { id, editorSession, adminSession, readKey, registerKey };
}

inv("inv_node_01", "変換関数が定義されていないカスタムノードを含むスキーマは、CLIを通さずにデプロイのAPIを直接呼んだ場合もデプロイが拒否される", async () => {
  const { cms } = await world();
  const name = templateName("node01");
  const attrs = [{ name: "label", type: "string" as const }];
  expectSuccess(await deployRaw(withNodes(name, [])), "カスタムノードのないテンプレートのデプロイ");
  const site = await siteOn(name);
  const schemaText = async () => (await cms.schemaInfoRaw(asSession(site.adminSession), site.id)).text;

  const refused: [string, CustomNodeSpec[]][] = [
    ["変換関数のないブロックのカスタムノード", [{ name: "callout", attrs }]],
    ["変換関数のないインラインのカスタムノード", [{ name: "callout", inline: true, attrs }]],
    ["HTMLへの変換関数だけがあるカスタムノード", [{ name: "callout", attrs, toHtml: customToHtml("<b>x</b>") }]],
    ["カスタムノードの宣言を通さず、本文のノードとして直接書いたノード", [{ name: "callout", attrs, asPlainBodyNode: true }]],
    [
      "変換関数のあるノードとないノードの組み合わせ",
      [
        { name: "note", attrs, toMarkdown: customToMarkdown() },
        { name: "callout", attrs },
      ],
    ],
  ];
  for (const [what, nodes] of refused) {
    expectRefused(await deployRaw(withNodes(name, nodes)), what);
    expect(/callout/.test(await schemaText()), `${what}のデプロイが拒否された後に、スキーマが変わった`).toBe(false);
  }

  // 対照: 変換関数があればデプロイでき、そのノードを本文に書ける
  expectSuccess(await deployRaw(withNodes(name, [{ name: "callout", attrs, toMarkdown: customToMarkdown() }])), "変換関数のあるカスタムノードのデプロイ");
  expect(/callout/.test(await schemaText()), "デプロイしたカスタムノードがスキーマにない").toBe(true);
  const body = doc(paragraph("前"), { type: "callout", attrs: { label: "ラベル" } }, paragraph("後"));
  expectSuccess(await cms.createEntryRaw(asSession(site.editorSession), site.id, { title: marker("node01"), body }), "カスタムノードを含む本文の保存");
});

inv("inv_cnv_01", "変換器が出力するHTMLでは、本文中の文字列がすべてエスケープされ、スクリプトを実行できる要素や属性が含まれない", async () => {
  const { cms } = await world();
  const m = marker("cnv");
  // 本文に書かれる、HTMLとして解釈されると危険な文字列（どれも m を含むので、そのまま出たかを見分けられる）
  const nasty = [
    `<script>alert("${m}")</script>`,
    `"><img src=x onerror=alert('${m}')>`,
    `<svg onload=alert(1)>${m}</svg>`,
    `</p><iframe src="javascript:alert(1)">${m}`,
    `<a href="javascript:alert(1)">${m}</a>`,
    `<style>*{background:url(javascript:alert(1))}</style>${m}`,
  ];
  // カスタムノードのHTMLへの変換関数が返す、危険なHTML（関数の書き手の誤りや悪意）
  const safe = marker("safe");
  const evilHtml = [
    `<strong>${safe}</strong>`,
    `<script>alert(1)</script>`,
    `<img src=x onerror=alert(1)>`,
    `<a href="javascript:alert(1)">a</a>`,
    `<a href="java&#x09;script:alert(1)">b</a>`,
    `<a href="data:text/html,<script>alert(1)</script>">c</a>`,
    `<div style="background:url(javascript:alert(1))" onclick="alert(1)">d</div>`,
    `<svg><animate onbegin="alert(1)" attributeName="x"/></svg>`,
    `<math><annotation-xml encoding="text/html"><script>alert(1)</script></annotation-xml></math>`,
    `<iframe srcdoc="<script>alert(1)</script>"></iframe>`,
    `<form action="javascript:alert(1)"><button>e</button></form>`,
    `<!--<script>alert(1)</script>-->`,
    `<noscript><p title="</noscript><img src=x onerror=alert(1)>"></noscript>`,
  ].join("");
  const attrs = [{ name: "label", type: "string" as const }];
  const name = templateName("cnv01");
  expectSuccess(
    await deployRaw(
      withNodes(
        name,
        [
          { name: "callout", attrs, toMarkdown: customToMarkdown(), toHtml: customToHtml(evilHtml) },
          { name: "badge", inline: true, attrs, toMarkdown: customToMarkdown(), toHtml: customToHtml(`<em>${safe}-inline</em><img src=x onerror=alert(1)>`) },
          // HTMLへの変換関数のないカスタムノード（変換器が既定の形で出す）
          { name: "note", attrs, toMarkdown: customToMarkdown() },
        ],
        (t) => {
          t.bodyNodes = [...t.bodyNodes, "codeBlock", "blockquote"];
        },
      ),
    ),
    "テンプレートのデプロイ",
  );
  const site = await siteOn(name);
  const media = await cms.uploadMedia(site.editorSession, site.id, { filename: "cnv.png", bytes: pngImage(8, 8), contentType: "image/png" });

  const callout = (label: string): TiptapNode => ({ type: "callout", attrs: { label } });
  const body = doc(
    heading(nasty[0]!),
    paragraph(nasty[1]!, text(nasty[2]!, [{ type: "bold" }]), link(nasty[3]!, `https://example.com/?q="><script>alert(1)</script>${m}`)),
    paragraph(link("anchor", `#"><img src=x onerror=alert(1)>`), { type: "badge", attrs: { label: nasty[4]! } }),
    { type: "codeBlock", attrs: { language: `"><script>alert(1)</script>` }, content: [text(nasty[5]!)] },
    { type: "blockquote", content: [paragraph(nasty[0]!)] },
    driver.body.image({ mediaId: media.id, alt: `"onerror="alert(1)" x="${m}` }),
    driver.body.embed("https://www.youtube.com/embed/aaaaaaaaaaa"),
    callout(nasty[1]!),
    { type: "note", attrs: { label: nasty[0]! } },
  );
  const { id } = await cms.createEntry(site.editorSession, site.id, { title: marker("cnv01"), body });
  await cms.publishEntry(site.editorSession, site.id, id);

  const query = await cms.registerQuery(site.id, site.registerKey.secret, { kind: "byId", include: ["bodyHtml"] });
  const res = await cms.deliveryRaw(site.id, site.readKey.secret, query, { id, locale: "ja" });
  expectSuccess(res, "本文のHTMLの配信");
  const html = findString(JSON.parse(res.text) as unknown, "bodyHtml");
  expect(html, "配信APIの応答に本文のHTMLがない").toBeTypeOf("string");

  // 本文中の文字列は、エスケープされて出る（文字列そのものは残り、HTMLとしては解釈されない）
  expect(html!.includes(m), "本文中の文字列がHTMLに出ていない（変換されていない）").toBe(true);
  for (const s of nasty) {
    expect(html!.includes(s), `本文中の文字列がエスケープされずに出ている: ${s}`).toBe(false);
  }
  // スクリプトを実行できる要素や属性がない（カスタムノードの関数が返したHTMLを含む）
  expectNoActiveHtml(html!, "変換器が出力したHTML");

  // 対照: ふつうの構造と、カスタムノードの関数が返した安全な部分は残る
  const tags = tagsOf(html!).map((t) => t.toLowerCase());
  for (const el of ["<h2", "<p", "<a", "<pre", "<blockquote", "<img"]) {
    expect(tags.some((t) => t.startsWith(el)), `HTMLに ${el}> がない`).toBe(true);
  }
  expect(new RegExp(`<strong[^>]*>${safe}</strong>`).test(html!), "カスタムノードの関数が返した安全な部分が消えた").toBe(true);
  expect(html!.includes(`${safe}-inline`), "インラインのカスタムノードの関数が返した安全な部分が消えた").toBe(true);
});
