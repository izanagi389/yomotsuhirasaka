// 本文（TiptapのJSON）の組み立て。Tiptapの標準のノード（StarterKitとLink）だけをここで作る。
// CMS固有のノード（画像・埋め込み）と、見出しのアンカーIDの付け方はドライバーが持つ（driver.body）。

export interface TiptapMark {
  type: string;
  attrs?: Record<string, unknown>;
}

export interface TiptapNode {
  type: string;
  attrs?: Record<string, unknown>;
  content?: TiptapNode[];
  marks?: TiptapMark[];
  text?: string;
}

export interface TiptapDoc {
  type: "doc";
  content: TiptapNode[];
}

export const text = (value: string, marks?: TiptapMark[]): TiptapNode => (marks ? { type: "text", text: value, marks } : { type: "text", text: value });

export const paragraph = (...children: (string | TiptapNode)[]): TiptapNode => ({
  type: "paragraph",
  content: children.map((c) => (typeof c === "string" ? text(c) : c)),
});

export const link = (label: string, href: string): TiptapNode => text(label, [{ type: "link", attrs: { href } }]);

export const heading = (value: string, level = 2): TiptapNode => ({ type: "heading", attrs: { level }, content: [text(value)] });

export const doc = (...nodes: TiptapNode[]): TiptapDoc => ({ type: "doc", content: nodes });

/** 段落だけの本文。 */
export const simpleBody = (...paragraphs: string[]): TiptapDoc => doc(...paragraphs.map((p) => paragraph(p)));

/** 本文の中の見出しを、出てくる順に返す。 */
export function headingsOf(d: TiptapDoc): TiptapNode[] {
  const out: TiptapNode[] = [];
  const walk = (n: TiptapNode) => {
    if (n.type === "heading") out.push(n);
    n.content?.forEach(walk);
  };
  d.content.forEach(walk);
  return out;
}

export const headingText = (h: TiptapNode): string => (h.content ?? []).map((c) => c.text ?? "").join("");
