// メディアの約束: INV-MED-01〜06

import { mkdtempSync, readdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { driver, type UploadFile } from "./support/driver.js";
import { expectRefused, expectSuccess } from "./support/assert.js";
import { env } from "./support/env.js";
import { isSuccess } from "./support/http.js";
import {
  AVIF_IMAGE,
  exifWithGps,
  GIF_JS_POLYGLOT,
  GPS_MARKER,
  hasGpsExif,
  HTML_PAGE,
  jpegImage,
  PDF_STUB,
  pngHeaderOnlyHuge,
  pngImage,
  pngNoise,
  pngPixelBomb,
  SVG_WITH_SCRIPT,
} from "./support/images.js";
import { lazyWorld } from "./support/world.js";

const world = lazyWorld();

const file = (filename: string, bytes: Buffer, contentType: string): UploadFile => ({ filename, bytes, contentType });

/** 画像として知られた形式の先頭のバイト列か。 */
const looksLikeImage = (b: Buffer): boolean =>
  b.subarray(0, 8).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])) ||
  (b[0] === 0xff && b[1] === 0xd8) ||
  b.subarray(0, 4).toString("latin1") === "GIF8" ||
  (b.subarray(0, 4).toString("latin1") === "RIFF" && b.subarray(8, 12).toString("latin1") === "WEBP");

/** ディレクトリの下を再帰的にたどり、名前に token を含むファイルとディレクトリを返す。 */
function findNamed(root: string, token: string): string[] {
  const found: string[] = [];
  const walk = (dir: string) => {
    let entries;
    try {
      entries = readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const e of entries) {
      const p = join(dir, e.name);
      if (e.name.includes(token)) found.push(p);
      if (e.isDirectory() && !e.isSymbolicLink()) walk(p);
    }
  };
  walk(root);
  return found;
}

inv("inv_med_01", "中身が許可した形式でないファイルは、拡張子にかかわらずアップロードを拒否される。SVGは既定で、AVIFは常に拒否される", async () => {
  const { yomi, a } = await world();
  const s = a.editorSession;
  const refused: [string, UploadFile][] = [
    ["拡張子が .png のHTML", file("page.png", HTML_PAGE, "image/png")],
    ["拡張子が .jpg のPDF", file("doc.jpg", PDF_STUB, "image/jpeg")],
    ["SVG", file("icon.svg", SVG_WITH_SCRIPT, "image/svg+xml")],
    ["拡張子が .png のSVG", file("icon.png", SVG_WITH_SCRIPT, "image/png")],
    ["AVIF", file("photo.avif", AVIF_IMAGE, "image/avif")],
    ["拡張子が .png のAVIF", file("photo.png", AVIF_IMAGE, "image/png")],
    ["拡張子が .webp のAVIF", file("photo.webp", AVIF_IMAGE, "image/webp")],
  ];
  for (const [label, f] of refused) expectRefused(await yomi.uploadMediaRaw(s, a.id, f), `${label}のアップロード`);

  // 対照: 中身がPNG・JPEGなら、拡張子や申告の形式が違っても受け付ける
  expectSuccess(await yomi.uploadMediaRaw(s, a.id, file("photo.png", pngImage(8, 8), "image/png")), "PNGのアップロード");
  expectSuccess(await yomi.uploadMediaRaw(s, a.id, file("photo.jpg", jpegImage(), "image/jpeg")), "JPEGのアップロード");
  expectSuccess(await yomi.uploadMediaRaw(s, a.id, file("photo.txt", pngImage(8, 8), "text/plain")), "拡張子が .txt のPNGのアップロード");
});

inv("inv_med_02", "アップロードした画像の位置情報は、既定で削除される", async () => {
  const { yomi, a } = await world();
  for (const [label, f] of [
    ["JPEG", file("gps.jpg", jpegImage({ exif: exifWithGps() }), "image/jpeg")],
    ["PNG", file("gps.png", pngImage(16, 16, { exif: exifWithGps() }), "image/png")],
  ] as const) {
    expect(hasGpsExif(f.bytes), `テスト用の${label}に位置情報が入っていない`).toBe(true);
    const media = await yomi.uploadMedia(a.editorSession, a.id, f);
    const fetched = await yomi.fetchMediaRaw(media.url);
    expectSuccess(fetched, `${label}の取得`);
    expect(looksLikeImage(fetched.bytes), `保存された${label}が画像として読めない`).toBe(true);
    expect(hasGpsExif(fetched.bytes), `保存された${label}に位置情報が残っている`).toBe(false);
    expect(fetched.bytes.includes(GPS_MARKER), `保存された${label}に位置情報の目印が残っている`).toBe(false);
  }
});

inv("inv_med_03", "メディアの応答には、スクリプトの実行を禁じるヘッダーが付く", async () => {
  const { yomi, a } = await world();
  for (const [label, f] of [
    ["PNG", file("h.png", pngImage(8, 8), "image/png")],
    ["JPEG", file("h.jpg", jpegImage(), "image/jpeg")],
    ["GIFとスクリプトを兼ねるファイル", file("h.gif", GIF_JS_POLYGLOT, "image/gif")],
  ] as const) {
    const up = await yomi.uploadMediaRaw(a.editorSession, a.id, f);
    if (!isSuccess(up)) continue; // 受け付けないなら配信もされない
    const media = await yomi.uploadMedia(a.editorSession, a.id, f);
    const res = await yomi.fetchMediaRaw(media.url);
    expectSuccess(res, `${label}の取得`);
    expect(res.headers.get("x-content-type-options")?.toLowerCase(), `${label}の X-Content-Type-Options`).toBe("nosniff");
    const csp = (res.headers.get("content-security-policy") ?? "").toLowerCase();
    expect(/(^|;)\s*sandbox(\s|;|$)/.test(csp), `${label}のCSPに sandbox がない: ${csp}`).toBe(true);
  }
});

inv("inv_med_04", "利用者が指定したファイル名やパス（取り込みを含む）によって、保存先の外にファイルを書けない", async () => {
  const { yomi, a } = await world();
  const token = `invmed04${randomUUID().replace(/-/g, "").slice(0, 12)}`;
  const names = [
    `../../${token}-a.png`,
    `..\\..\\${token}-b.png`,
    `/tmp/${token}-c.png`,
    `%2e%2e%2f%2e%2e%2f${token}-d.png`,
    `....//....//${token}-e.png`,
    `${token}/../../../f.png`,
  ];
  for (const name of names) {
    const res = await yomi.uploadMediaRaw(a.editorSession, a.id, file(name, pngImage(8, 8), "image/png"));
    if (isSuccess(res)) expect(res.text.includes(token), `保存されたメディアの情報に、指定したファイル名（${name}）が使われている`).toBe(false);
  }

  // 取り込み: 画像のファイル名に目印を入れる
  const source = mkdtempSync(join(tmpdir(), "inv-med04-"));
  writeFileSync(join(source, `${token}-import.png`), pngImage(8, 8));
  writeFileSync(join(source, "post.md"), `---\ntitle: 取り込み\nslug: med04-${token}\nlocale: ja\nurl: https://old.inv.test/med04\n---\n\n![画像](./${token}-import.png)\n`);
  const importToken = await yomi.createPersonalToken(a.adminSession, [a.id], { scopes: ["read", "write"] });
  const result = await driver.runImport(yomi.baseUrl, {
    sourceDir: source,
    workspaceId: a.id,
    token: importToken.secret,
    originalUrlKey: "url",
    redirectOutput: join(source, "redirects.json"),
  });
  expect(result.exitCode, `取り込みが失敗した:\n${result.output}`).toBe(0);

  // CMSのファイルシステムのどこにも、指定した名前のファイルができていない
  expect(findNamed(env.fsRoot, token), "指定したファイル名のファイルが作られている").toEqual([]);
});

inv("inv_med_05", "ワークスペースの容量の上限を超えるアップロードは拒否される", async () => {
  const { yomi, serverAdmin, serverAdminSession } = await world();
  await yomi.reauthenticate(serverAdminSession, serverAdmin);
  const ws = await yomi.createWorkspace(serverAdminSession, `inv-med-05-${randomUUID().slice(0, 6)}`);
  const editor = await yomi.createMember(serverAdminSession, ws.id, "editor");
  const session = await yomi.login(editor);
  await yomi.setWorkspaceLimits(serverAdminSession, ws.id, { mediaBytes: 200_000 });

  const image = pngNoise(150, 150); // 圧縮の効かない約67KB
  expectSuccess(await yomi.uploadMediaRaw(session, ws.id, file("1.png", image, "image/png")), "上限の内側のアップロード");
  let refused = false;
  for (let i = 0; i < 12 && !refused; i++) {
    const res = await yomi.uploadMediaRaw(session, ws.id, file(`${i + 2}.png`, pngNoise(150, 150 + i), "image/png"));
    if (!isSuccess(res)) {
      expectRefused(res, "容量の上限を超えるアップロード");
      refused = true;
    }
  }
  expect(refused, "容量の上限（200KB）の数倍をアップロードしても拒否されない").toBe(true);
});

inv("inv_med_06", "画素数の上限を超える画像は、ファイルの大きさにかかわらずアップロードを拒否される", async () => {
  const { yomi, a } = await world();
  const bomb = pngPixelBomb();
  expect(bomb.length, "解凍爆弾のファイルが大きすぎる").toBeLessThan(200_000);
  expectRefused(await yomi.uploadMediaRaw(a.editorSession, a.id, file("bomb.png", bomb, "image/png")), "4億画素のPNG（ファイルは小さい）");
  expectRefused(await yomi.uploadMediaRaw(a.editorSession, a.id, file("huge.png", pngHeaderOnlyHuge(), "image/png")), "100億画素を宣言するPNG");
  // 対照: 普通の大きさの画像は受け付ける
  expectSuccess(await yomi.uploadMediaRaw(a.editorSession, a.id, file("ok.png", pngImage(100, 100), "image/png")), "100×100のPNG");
});
