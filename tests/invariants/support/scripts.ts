// ドライバーの一部: 約束のテストで、組み込みJSで動かす関数（URL生成関数など）のソース。
//
// ホスト関数の呼び方（`cms.content.get(id)` が `{id, contentType, locale, fields}` か null を返す）・関数の受け取る値
// （記事の locale と slug）・モジュールの書き方（既定のエクスポート）は、実装の細部としてここに閉じ込める
// （server/src/script/ に合わせる）。
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
