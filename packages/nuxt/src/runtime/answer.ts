// チャットボットの回答の、画面とサーバーの両方で使うもの（型・上限・イベントの読み取り・Markdown の一部の解釈）。
// Nuxt にも Nitro にも依存しない（09-qa-chatbot.md、13-security.md「チャットボットへの悪用」）。
// - 回答は Markdown。表示する前にエスケープする（v-html は使わない）。描けるのは段落・箇条書き・太字・リンク・インラインのコード・
//   コードのかたまりだけ。画像は描かない。リンクの URL は描く前にも確かめる（CMS も回答の URL を絞り込んでいる）
// - 本文の `[n]` は引用の記号。出典の `citation` と同じ番号の出典へのリンクにできる

import { hrefAllowed } from "./href.ts";

/** 質問文の長さの上限（CMSと同じ）。 */
export const MAX_QUESTION_CHARS = 500;

/** 回答の出典（記事の該当セクション）。 */
export interface YomiAnswerSource {
  /** 本文の引用記号 `[n]` の番号。 */
  citation: number;
  entryId: string;
  locale: string;
  /** 記事の公開URL（アンカーは付かない）。 */
  url: string | null;
  anchor: string | null;
  breadcrumb: string[];
  versionId: string;
  lastReviewedAt: string | null;
  /** 確認期限切れか（断定を避けた回答になっている）。 */
  reviewExpired: boolean;
}

/** 最終判定（ANSWERED・NO_INFORMATION（記事に答えがない）・VIOLATION_DETECTED（本文の途中で規則が破られた。最終の本文を出す））。 */
export type YomiAnswerStatus = "ANSWERED" | "NO_INFORMATION" | "VIOLATION_DETECTED";

export interface YomiAnswer {
  status: YomiAnswerStatus;
  /** 回答の本文（Markdown）。NO_INFORMATION ならワークスペースの定型文。 */
  text: string;
  /** 長さの上限で打ち切ったか。 */
  truncated: boolean;
  sources: YomiAnswerSource[];
}

/**
 * ブラウザに渡すエラーのコード。CMS のメッセージは渡さない。
 * - RATE_LIMITED: 回数の制限（retryAfter 秒後にまた送れる）
 * - ANSWER_BUDGET_EXHAUSTED: ワークスペースの回答の予算切れ
 * - QUESTION_EMPTY・QUESTION_TOO_LONG・QUESTION_REFUSED（ワークスペースが答えないと決めた質問）・LOCALE_NOT_SUPPORTED
 * - LLM_UNAVAILABLE: LLM を呼べなかった（時間をおいて送り直す）
 * - ANSWER_FAILED: そのほか（CMS に届かない・サイトの設定の誤りなど）
 */
export type YomiAnswerErrorCode =
  | "RATE_LIMITED"
  | "ANSWER_BUDGET_EXHAUSTED"
  | "QUESTION_EMPTY"
  | "QUESTION_TOO_LONG"
  | "QUESTION_REFUSED"
  | "LOCALE_NOT_SUPPORTED"
  | "LLM_UNAVAILABLE"
  | "ANSWER_FAILED";

/** CMS の GraphQL のエラーのコードのうち、そのままブラウザに渡すもの。 */
export const PASSED_ERROR_CODES: readonly YomiAnswerErrorCode[] = [
  "ANSWER_BUDGET_EXHAUSTED",
  "QUESTION_EMPTY",
  "QUESTION_TOO_LONG",
  "QUESTION_REFUSED",
  "LOCALE_NOT_SUPPORTED",
  "LLM_UNAVAILABLE",
];

/**
 * サイトのサーバーのルートが送るイベント（SSE の `event:` が type、`data:` がそれ以外の項目の JSON）。
 * verdict（判定。answerable が false なら本文は来ない）→ delta（本文の断片）→ final（最終判定）。途中で error が来たら終わり。
 */
export type YomiAnswerEvent =
  | { type: "verdict"; answerable: boolean; sources: YomiAnswerSource[] }
  | { type: "delta"; text: string }
  | { type: "final"; answer: YomiAnswer }
  | { type: "error"; code: YomiAnswerErrorCode; retryAfter?: number };

/** SSE の本文を少しずつ受け取り、まとまったイベント（`event:` と `data:`）を返す。 */
export class SseReader {
  private buf = "";

  /** 受け取った文字列を足し、終わりまで届いたイベントを返す。 */
  push(chunk: string): { event: string; data: string }[] {
    this.buf += chunk;
    const out: { event: string; data: string }[] = [];
    for (;;) {
      const m = /\r?\n\r?\n/.exec(this.buf);
      if (!m) break;
      const block = this.buf.slice(0, m.index);
      this.buf = this.buf.slice(m.index + m[0].length);
      let event = "message";
      const data: string[] = [];
      for (const line of block.split(/\r?\n/)) {
        if (line.startsWith("event:")) event = line.slice(6).trim();
        else if (line.startsWith("data:")) data.push(line.slice(5).replace(/^ /, ""));
      }
      if (data.length > 0 || event !== "message") out.push({ event, data: data.join("\n") });
    }
    return out;
  }
}

// --- Markdown の一部 ---

/** 行の中の要素。 */
export type YomiAnswerInline =
  | { type: "text"; text: string }
  | { type: "strong"; children: YomiAnswerInline[] }
  | { type: "code"; text: string }
  | { type: "link"; href: string; children: YomiAnswerInline[] }
  | { type: "citation"; n: number };

/** かたまり。見出しは段落（太字）にする。 */
export type YomiAnswerBlock =
  | { type: "paragraph"; children: YomiAnswerInline[] }
  | { type: "list"; ordered: boolean; items: YomiAnswerInline[][] }
  | { type: "code"; text: string };

/** 回答の Markdown を、描いてよい要素だけの木にする（知らない書き方は文字のまま）。 */
export function parseAnswerMarkdown(text: string): YomiAnswerBlock[] {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  const blocks: YomiAnswerBlock[] = [];
  let para: string[] = [];
  const flush = () => {
    if (para.length > 0) blocks.push({ type: "paragraph", children: parseInline(para.join("\n")) });
    para = [];
  };
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]!;
    // コードのかたまり
    if (/^\s*```/.test(line)) {
      flush();
      const code: string[] = [];
      for (i++; i < lines.length && !/^\s*```/.test(lines[i]!); i++) code.push(lines[i]!);
      blocks.push({ type: "code", text: code.join("\n") });
      continue;
    }
    if (line.trim() === "") {
      flush();
      continue;
    }
    // 箇条書き（同じ種類の行が続く間）
    const item = /^\s*([-*+]|\d{1,9}[.)])\s+(.*)$/.exec(line);
    if (item) {
      flush();
      const ordered = /\d/.test(item[1]!);
      const items: YomiAnswerInline[][] = [];
      for (; i < lines.length; i++) {
        const m = /^\s*([-*+]|\d{1,9}[.)])\s+(.*)$/.exec(lines[i]!);
        if (!m || /\d/.test(m[1]!) !== ordered) break;
        items.push(parseInline(m[2]!));
      }
      i--;
      blocks.push({ type: "list", ordered, items });
      continue;
    }
    // 見出しは太字の段落に
    const heading = /^\s{0,3}#{1,6}\s+(.*?)\s*#*\s*$/.exec(line);
    if (heading) {
      flush();
      blocks.push({ type: "paragraph", children: [{ type: "strong", children: parseInline(heading[1]!) }] });
      continue;
    }
    // 引用は、ふつうの段落として
    para.push(line.replace(/^\s*>\s?/, ""));
  }
  flush();
  return blocks;
}

/** 行の中を解釈する。 */
export function parseInline(text: string): YomiAnswerInline[] {
  const out: YomiAnswerInline[] = [];
  const pushText = (t: string) => {
    if (!t) return;
    const last = out[out.length - 1];
    if (last?.type === "text") last.text += t;
    else out.push({ type: "text", text: t });
  };
  let i = 0;
  while (i < text.length) {
    const rest = text.slice(i);
    // 画像は描かない（説明の文も出さない）
    const image = /^!\[[^\]\n]*\]\([^)\n]*\)/.exec(rest);
    if (image) {
      i += image[0].length;
      continue;
    }
    // リンク（描いてよい URL だけ。だめなら文字だけ）
    const link = /^\[([^\]\n]+)\]\(([^)\s]+)(?:\s+"[^"\n]*")?\)/.exec(rest);
    if (link) {
      const children = parseInline(link[1]!);
      if (hrefAllowed(link[2]) && /^https?:\/\//i.test(link[2]!)) out.push({ type: "link", href: link[2]!, children });
      else
        for (const c of children) {
          if (c.type === "text") pushText(c.text);
          else out.push(c);
        }
      i += link[0].length;
      continue;
    }
    // 引用の記号（後ろに `(` が続かない `[数字]`）
    const citation = /^\[(\d{1,3})\](?!\()/.exec(rest);
    if (citation) {
      out.push({ type: "citation", n: Number(citation[1]) });
      i += citation[0].length;
      continue;
    }
    // 自動リンクの書き方（`<https://…>`）は文字のまま（山かっこは外す）
    const auto = /^<(https?:\/\/[^>\s]+)>/i.exec(rest);
    if (auto) {
      pushText(auto[1]!);
      i += auto[0].length;
      continue;
    }
    const code = /^`([^`\n]+)`/.exec(rest);
    if (code) {
      out.push({ type: "code", text: code[1]! });
      i += code[0].length;
      continue;
    }
    const strong = /^(\*\*|__)(?=\S)([\s\S]*?\S)\1/.exec(rest);
    if (strong) {
      out.push({ type: "strong", children: parseInline(strong[2]!) });
      i += strong[0].length;
      continue;
    }
    pushText(text[i]!);
    i += 1;
  }
  return out;
}

/** 出典へのリンク先（記事のURLと、セクションのアンカー）。描いてよいURLでなければ null。 */
export function sourceHref(source: YomiAnswerSource | undefined): string | null {
  if (!source?.url) return null;
  const href = source.anchor ? `${source.url}#${encodeURIComponent(source.anchor)}` : source.url;
  return hrefAllowed(href) && /^https?:\/\//i.test(href) ? href : null;
}

// --- 画面の状態 ---

/**
 * 回答の状態。asking は送ってから判定が届くまで、streaming は本文が届いている間、done は最終判定が届いた後。
 * error のとき、error.code（と、回数の制限なら retryAfter 秒）を見る。
 */
export type YomiAnswerPhase = "idle" | "asking" | "streaming" | "done" | "error";

export interface YomiAnswerState {
  phase: YomiAnswerPhase;
  /** 判定（届くまで null）。false なら本文は来ず、最終判定の本文が「情報なし」の定型文になる。 */
  answerable: boolean | null;
  /** いまの本文（届いた断片をつないだもの。最終判定が届いたら、その本文に置き換える）。 */
  text: string;
  sources: YomiAnswerSource[];
  /** 最終判定（届いた後だけ）。 */
  answer: YomiAnswer | null;
  error: { code: YomiAnswerErrorCode; retryAfter?: number } | null;
}

export const INITIAL_ANSWER_STATE: YomiAnswerState = { phase: "idle", answerable: null, text: "", sources: [], answer: null, error: null };

/** イベントを1つ受け取った後の状態。 */
export function applyAnswerEvent(state: YomiAnswerState, event: YomiAnswerEvent): YomiAnswerState {
  switch (event.type) {
    case "verdict":
      return { ...state, phase: event.answerable ? "streaming" : "asking", answerable: event.answerable, sources: event.sources };
    case "delta":
      return { ...state, phase: "streaming", text: state.text + event.text };
    case "final":
      return { ...state, phase: "done", text: event.answer.text, sources: event.answer.sources, answer: event.answer };
    case "error":
      return { ...state, phase: "error", error: { code: event.code, ...(event.retryAfter === undefined ? {} : { retryAfter: event.retryAfter }) } };
  }
}
