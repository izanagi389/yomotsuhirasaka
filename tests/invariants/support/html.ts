// 変換器が出力したHTMLの確認（INV-CNV-01）。HTMLのパーサーは使わず、出てはいけないものを保守的に探す。
// エスケープされた文字列（&lt;script&gt; など）はタグとして数えない。

import { expect } from "vitest";

/** スクリプトを実行できる、または外部の内容を読み込める要素。 */
const ACTIVE_ELEMENTS = [
  "script", "iframe", "frame", "frameset", "object", "embed", "applet", "svg", "math", "style", "link", "meta", "base",
  "form", "input", "button", "textarea", "select", "option", "template", "noscript", "portal", "video", "audio", "source",
];

/** 値にURLを取る属性。 */
const URL_ATTRS = ["href", "src", "action", "formaction", "xlink:href", "poster", "background", "cite", "data", "srcset"];

/** 属性の値の文字参照を戻し、空白と制御文字を除く（`java&#x09;script:` のような書き方も見つけるため）。 */
function normalizeValue(v: string): string {
  return v
    .replace(/&#x([0-9a-f]+);?/gi, (_, h: string) => String.fromCodePoint(parseInt(h, 16)))
    .replace(/&#([0-9]+);?/g, (_, d: string) => String.fromCodePoint(parseInt(d, 10)))
    .replace(/&colon;/gi, ":")
    .replace(/&tab;/gi, "\t")
    .replace(/&newline;/gi, "\n")
    .replace(/[\s\u0000-\u001f]/g, "")
    .toLowerCase();
}

/** タグ（`<名前 ...>` と `</名前>`）の一覧。 */
export function tagsOf(html: string): string[] {
  return html.match(/<\/?[a-zA-Z][^>]*>/g) ?? [];
}

/** スクリプトを実行できる要素や属性が、HTMLに含まれないことを確かめる。 */
export function expectNoActiveHtml(html: string, what: string): void {
  expect(/<!--|<!\[cdata\[|<\?/i.test(html), `${what}: コメントや処理命令が含まれている`).toBe(false);
  for (const tag of tagsOf(html)) {
    const name = /^<\/?([a-zA-Z][a-zA-Z0-9:-]*)/.exec(tag)![1]!.toLowerCase();
    expect(ACTIVE_ELEMENTS.includes(name), `${what}: 要素 <${name}> が含まれている: ${tag}`).toBe(false);
    expect(/[\s/"']on[a-z]+\s*=/i.test(tag), `${what}: イベントの属性が含まれている: ${tag}`).toBe(false);
    expect(/[\s/"'](style|srcdoc)\s*=/i.test(tag), `${what}: style や srcdoc の属性が含まれている: ${tag}`).toBe(false);
    for (const m of tag.matchAll(/[\s/"']([a-zA-Z:-]+)\s*=\s*("([^"]*)"|'([^']*)'|([^\s>]+))/g)) {
      const attr = m[1]!.toLowerCase();
      if (!URL_ATTRS.includes(attr)) continue;
      const value = normalizeValue(m[3] ?? m[4] ?? m[5] ?? "");
      expect(/^(javascript|vbscript|data|file):/.test(value), `${what}: 危険なスキームのURLが含まれている: ${tag}`).toBe(false);
    }
  }
}

/** 応答のJSONの中から、指定した名前の文字列の値を探す（配信APIの応答の形によらないため）。 */
export function findString(json: unknown, key: string): string | undefined {
  if (Array.isArray(json)) {
    for (const v of json) {
      const found = findString(v, key);
      if (found !== undefined) return found;
    }
  } else if (json !== null && typeof json === "object") {
    for (const [k, v] of Object.entries(json)) {
      if (k === key && typeof v === "string") return v;
      const found = findString(v, key);
      if (found !== undefined) return found;
    }
  }
  return undefined;
}
