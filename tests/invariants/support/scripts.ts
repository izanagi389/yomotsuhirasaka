// ドライバーの一部: 約束のテストで、組み込みJSで動かす関数（URL生成関数など）のソース。
//
// 次の実装の細部は、ここに閉じ込める（server/src/script/ に合わせる）。
// - モジュールの書き方: 既定のエクスポートが関数本体
// - ホスト関数: `cms.content.get(id)` は `{id, contentType, locale, fields}` か null、
//   `cms.http.fetch(url, {method, headers, body})` は `{status, body}` を返す（失敗は例外）
// - URL生成関数の受け取る値: `{id, contentType, locale, slug, fields}`。返す値: URLの文字列
// - 同期のフックの受け取る値: `{entry: {id, contentType, locale, fields}}`。
//   beforeSave は `{fields: {...}}`（直すフィールド）、beforePublish は `{errors: [{field?, message}]}` を返す
// - 非同期のフックの受け取る値: `{event: {id, type, workspaceId, entryId, locale, versionId, url}, entry}`。返す値は使わない
// ここに書くのは「関数が何をするか」だけで、合否の判定は書かない。

const lit = (v: unknown): string => JSON.stringify(v);

/** 公開URLの共通の頭。テストのテンプレートのURLと同じサイトにする。 */
const SITE = "https://site.inv.test";

/**
 * 記事を読むURL生成関数。
 *
 * ids の記事をホスト関数で読み、読めた記事のタイトルをURLの1段目に並べる（読めなければ `none`、
 * 呼び出しが例外になれば `denied`）。2段目には、前回の実行が残したグローバル変数の値（なければ `empty`）を入れ、
 * 今回の結果をグローバル変数に残す。実行環境が他のワークスペースと共有されていれば、2段目にその記事のタイトルが出る。
 *
 * タイトルはURLにそのまま入るよう、英数字とハイフンだけにしておくこと。
 */
export function readingUrlFunction(ids: string[]): string {
  return `
const IDS = ${lit(ids)};
function titleOf(id) {
  try {
    const e = cms.content.get(id);
    return e && e.fields && typeof e.fields.title === "string" ? e.fields.title : "none";
  } catch (_) {
    return "denied";
  }
}
export default function url(entry) {
  const found = IDS.map(titleOf).join("~");
  const previous = typeof globalThis.__invStash === "string" ? globalThis.__invStash : "empty";
  globalThis.__invStash = found;
  return ${lit(SITE)} + "/" + entry.locale + "/blog/" + found + "/" + previous + "/" + entry.slug;
}
`;
}

/** 上限を超える関数で、上限がなければ実行を終えたときにURLに入る印。 */
export const RUNAWAY_FINISHED = "runaway-finished";

/**
 * 上限を超えるURL生成関数。
 * - slug が `spin-` で始まる記事: spinMs ミリ秒のあいだ計算を続け、終われば RUNAWAY_FINISHED を含むURLを返す
 * - slug が `bomb-` で始まる記事: 1MBの文字列を bombMegabytes 個つくって持ち続け、終われば RUNAWAY_FINISHED を含むURLを返す
 * - それ以外: `/ok/` を含むふつうのURLを返す
 */
export function runawayUrlFunction(options: { spinMs: number; bombMegabytes: number }): string {
  return `
export default function url(entry) {
  const s = entry.slug;
  if (s.startsWith("spin-")) {
    const end = Date.now() + ${lit(options.spinMs)};
    let n = 0;
    while (Date.now() < end) { n++; }
    return ${lit(SITE)} + "/" + entry.locale + "/blog/${RUNAWAY_FINISHED}-" + (n > 0 ? "1" : "0") + "/" + s;
  }
  if (s.startsWith("bomb-")) {
    const keep = [];
    for (let i = 0; i < ${lit(options.bombMegabytes)}; i++) keep.push("x".repeat(1 << 20) + i);
    return ${lit(SITE)} + "/" + entry.locale + "/blog/${RUNAWAY_FINISHED}-" + keep.length + "/" + s;
  }
  return ${lit(SITE)} + "/" + entry.locale + "/blog/ok/" + s;
}
`;
}

// ---------------------------------------------------------------------------
// フック
// ---------------------------------------------------------------------------

/** 外部へ送ろうとする共通の部品。使えるものを片端から試し、失敗は捨てる（関数そのものは失敗させない）。 */
const TRY_SEND = `
function trySend(url, payload) {
  const body = JSON.stringify(payload);
  const attempts = [
    () => cms.http.fetch(url, { method: "POST", headers: { "content-type": "application/json" }, body }),
    () => fetch(url, { method: "POST", body }),
    () => { const x = new XMLHttpRequest(); x.open("POST", url, false); x.send(body); },
  ];
  for (const attempt of attempts) {
    try { attempt(); } catch (_) {}
  }
}
`;

/**
 * 保存前のフック: urls に送ろうとしてから、subtitle を tag に直す（フックが動いたことの印）。
 * 送れるかどうかにかかわらず、保存は止めない。
 */
export function beforeSaveSending(urls: string[], tag: string): string {
  return `${TRY_SEND}
export default function beforeSave(input) {
  for (const url of ${lit(urls)}) trySend(url, { entryId: input.entry.id });
  return { fields: { subtitle: ${lit(tag)} } };
}
`;
}

/** 公開前のフック: urls に送ろうとしてから、タイトルが `block-` で始まる記事の公開を止める（フックが動いたことの印）。 */
export function beforePublishSending(urls: string[]): string {
  return `${TRY_SEND}
export default function beforePublish(input) {
  for (const url of ${lit(urls)}) trySend(url, { entryId: input.entry.id });
  const title = String(input.entry.fields.title || "");
  return title.startsWith("block-") ? { errors: [{ field: "title", message: "blocked by the hook" }] } : { errors: [] };
}
`;
}

/** 非同期のフック: urls のそれぞれへ、記事のIDを付けて送る（1つ失敗しても残りを送る）。 */
export function afterSending(urls: string[]): string {
  return `${TRY_SEND}
export default function after(input) {
  for (const url of ${lit(urls)}) trySend(url, { entryId: input.event.entryId });
}
`;
}

/** 保存前のフック: ids の記事を読み、読めたタイトル（なければ none、例外なら denied）を subtitle に書く。 */
export function beforeSaveReading(ids: string[]): string {
  return `
function titleOf(id) {
  try {
    const e = cms.content.get(id);
    return e && e.fields && typeof e.fields.title === "string" ? e.fields.title : "none";
  } catch (_) {
    return "denied";
  }
}
export default function beforeSave(input) {
  return { fields: { subtitle: ${lit(ids)}.map(titleOf).join("~") } };
}
`;
}

/**
 * 上限を超える保存前のフック。タイトルが `spin-` で始まる記事では spinMs ミリ秒計算を続け、`bomb-` で始まる記事では
 * 1MBの文字列を bombMegabytes 個持ち続ける。終われば subtitle を RUNAWAY_FINISHED に直す。それ以外の記事では `ok` に直す。
 */
export function runawayBeforeSave(options: { spinMs: number; bombMegabytes: number }): string {
  return `
export default function beforeSave(input) {
  const title = String(input.entry.fields.title || "");
  if (title.startsWith("spin-")) {
    const end = Date.now() + ${lit(options.spinMs)};
    while (Date.now() < end) {}
    return { fields: { subtitle: ${lit(RUNAWAY_FINISHED)} } };
  }
  if (title.startsWith("bomb-")) {
    const keep = [];
    for (let i = 0; i < ${lit(options.bombMegabytes)}; i++) keep.push("x".repeat(1 << 20) + i);
    return { fields: { subtitle: ${lit(RUNAWAY_FINISHED)} + keep.length } };
  }
  return { fields: { subtitle: "ok" } };
}
`;
}

/**
 * 上限を超える公開後のフック。タイトルが `spin-` / `bomb-` で始まる記事では runawayBeforeSave と同じことをしてから、
 * finishedUrl に送る。それ以外の記事では、すぐに okUrl に送る。
 */
export function runawayAfterPublish(options: { spinMs: number; bombMegabytes: number; okUrl: string; finishedUrl: string }): string {
  return `${TRY_SEND}
export default function after(input) {
  const title = String((input.entry && input.entry.fields && input.entry.fields.title) || "");
  if (title.startsWith("spin-")) {
    const end = Date.now() + ${lit(options.spinMs)};
    while (Date.now() < end) {}
    trySend(${lit(options.finishedUrl)}, { entryId: input.event.entryId });
    return;
  }
  if (title.startsWith("bomb-")) {
    const keep = [];
    for (let i = 0; i < ${lit(options.bombMegabytes)}; i++) keep.push("x".repeat(1 << 20) + i);
    trySend(${lit(options.finishedUrl)}, { entryId: input.event.entryId, kept: keep.length });
    return;
  }
  trySend(${lit(options.okUrl)}, { entryId: input.event.entryId });
}
`;
}

// ---------------------------------------------------------------------------
// カスタムノードの変換関数（受け取るのはノードのJSON（Tiptap）と ctx。返すのは文字列）
// ---------------------------------------------------------------------------

/** Markdownへの変換関数: 属性 label をそのまま返す。 */
export function customToMarkdown(): string {
  return `
export default function toMarkdown(node, ctx) {
  return String((node.attrs && node.attrs.label) || "");
}
`;
}

/**
 * HTMLへの変換関数: html の後に、属性 label をエスケープせずに続ける。
 * 関数の書き手の誤りや悪意があっても、変換器の出力が無害化されることを確かめるため。
 */
export function customToHtml(html: string): string {
  return `
export default function toHtml(node, ctx) {
  return ${lit(html)} + String((node.attrs && node.attrs.label) || "");
}
`;
}
