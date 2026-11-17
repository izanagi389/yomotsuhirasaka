// 見出しの形の検査（構成の確認の a。10-llm-and-budget.md「編集者向けのAI支援」の4、2026-11-17）。
// AI を使わず、書いている間ずっと管理画面で行う。記事のタイトルを H1 とみなし、本文の見出しは H2 から始まる前提で、
// 段の飛び（H2 の次に H4 など）・空の見出し・同じ文の見出しの重なりを挙げる。

export interface HeadingProblem {
  kind: "empty" | "skip" | "duplicate";
  /** 見出しの文（empty では空）。 */
  text: string;
  /** skip: 前の見出しの段と、この見出しの段。 */
  from?: number;
  to?: number;
  /** duplicate: 同じ文の見出しの数。 */
  count?: number;
}

interface Node {
  type?: string;
  text?: string;
  attrs?: Record<string, unknown>;
  content?: Node[];
}

function plain(n: Node): string {
  return (n.text ?? "") + (n.content ?? []).map(plain).join("");
}

/** 本文（Tiptap の JSON。フィールドの順）の、最上位の見出しの段と文。 */
export function headingsOf(docs: unknown[]): { level: number; text: string }[] {
  const out: { level: number; text: string }[] = [];
  for (const d of docs) {
    const content = (d as Node | null)?.content;
    if (!Array.isArray(content)) continue;
    for (const n of content) {
      if (n?.type !== "heading") continue;
      const level = Number(n.attrs?.["level"] ?? 2);
      out.push({ level: Number.isFinite(level) ? level : 2, text: plain(n).trim() });
    }
  }
  return out;
}

/** 見出しの形の問題（出てきた順。重なりは2回目の位置で1回だけ）。 */
export function headingProblems(docs: unknown[]): HeadingProblem[] {
  const problems: HeadingProblem[] = [];
  const counts = new Map<string, number>();
  for (const h of headingsOf(docs)) if (h.text) counts.set(h.text, (counts.get(h.text) ?? 0) + 1);
  const reported = new Set<string>();
  let previous = 1; // 記事のタイトル
  for (const h of headingsOf(docs)) {
    if (!h.text) problems.push({ kind: "empty", text: "" });
    else if (h.level > previous + 1) problems.push({ kind: "skip", text: h.text, from: previous, to: h.level });
    const count = counts.get(h.text) ?? 0;
    if (h.text && count > 1 && !reported.has(h.text)) {
      reported.add(h.text);
      problems.push({ kind: "duplicate", text: h.text, count });
    }
    previous = h.level;
  }
  return problems;
}
