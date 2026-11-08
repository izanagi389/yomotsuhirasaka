// サイト内検索の、画面とサーバーの両方で使うもの（型・上限・一致した箇所の区切り）。Nuxt にも Nitro にも依存しない。

/** 検索のモード（配信APIの YomiSearchMode）。 */
export type YomiSearchMode = "HYBRID" | "VECTOR" | "KEYWORD";
export const SEARCH_MODES: readonly YomiSearchMode[] = ["HYBRID", "VECTOR", "KEYWORD"];
/** 検索語の長さの上限（CMSと同じ）。 */
export const MAX_QUERY_CHARS = 500;
export const DEFAULT_FIRST = 10;
export const MAX_FIRST = 50;

/** 検索のそれぞれの方式が使えたか（BUDGET_EXHAUSTED は埋め込みの予算切れ、DISABLED は日本語の全文検索の拡張がない）。 */
export type YomiSearchAvailability = "AVAILABLE" | "BUDGET_EXHAUSTED" | "DISABLED";

export interface YomiSearchHit {
  entryId: string;
  locale: string;
  /** 記事の公開URL（アンカーは付かない）。 */
  url: string | null;
  /** 当たったセクションのアンカーID。 */
  anchor: string | null;
  /** 記事タイトル（検索の対象のときだけ）＞大見出し＞小見出し。 */
  breadcrumb: string[];
  /** 当たったセクションの抜粋（プレーンテキスト）。 */
  excerpt: string;
  /** 抜粋の中で、キーワードに一致した箇所（UTF-16 の位置と長さ）。 */
  matches: { start: number; length: number }[];
  /** 1つの応答の中で並べるためだけの点数。 */
  score: number;
}

export interface YomiSearchResult {
  vectorSearch: YomiSearchAvailability;
  keywordSearch: YomiSearchAvailability;
  hits: YomiSearchHit[];
}

/** 抜粋を、一致した箇所とそれ以外に区切る。`<mark>` などで描くのに使う（`v-html` は使わない）。 */
export interface YomiHighlightSegment {
  text: string;
  match: boolean;
}

/**
 * `excerpt` を `matches` で区切る。範囲の外・重なり・逆順の一致は、使える部分だけを使う。
 * 位置と長さは UTF-16 の単位（JavaScript の文字列と同じ）。
 */
export function yomiHighlight(excerpt: string, matches: readonly { start: number; length: number }[] | null | undefined): YomiHighlightSegment[] {
  const out: YomiHighlightSegment[] = [];
  const push = (text: string, match: boolean) => {
    if (!text) return;
    const last = out[out.length - 1];
    if (last && last.match === match) last.text += text;
    else out.push({ text, match });
  };
  let at = 0;
  const sorted = [...(matches ?? [])].filter((m) => Number.isInteger(m.start) && Number.isInteger(m.length) && m.length > 0).sort((a, b) => a.start - b.start);
  for (const m of sorted) {
    const start = Math.max(m.start, at);
    const end = Math.min(m.start + m.length, excerpt.length);
    if (start >= end) continue;
    push(excerpt.slice(at, start), false);
    push(excerpt.slice(start, end), true);
    at = end;
  }
  push(excerpt.slice(at), false);
  return out;
}
