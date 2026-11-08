// 独自部品（MDXのJSXのタグ、Nuxt Content の MDC の `::name`）を見つけ、本文の変換の前に印に置き換える。
//
// - 設定の componentMap に対応がない部品があれば、取り込みの前に止める（INV-IMP-01）。
// - 対応がある部品は、ブロックの部品だけを扱う（行の途中の部品は、M1では対応しない）。
// - コードブロック（``` や ~~~）の中は見ない。HTMLのコメントの行は取り除く。

export type AttrValue = string | true;

export interface Component {
  name: string;
  attrs: Record<string, AttrValue>;
  /** 元のファイルの行番号（1から）。 */
  line: number;
}

export interface Prepared {
  /** 部品の始まりと終わりを、印（HTMLのコメント）に置き換えた本文。 */
  text: string;
  /** text の各行（0から）が、元のファイルの何行目か。 */
  lineOf: number[];
  components: Component[];
  problems: string[];
  warnings: string[];
}

/** 部品の始まりの印。markdown-it では1つのHTMLのブロックになる。 */
export const openMarker = (i: number) => `<!--yomi:component:${i}-->`;
export const closeMarker = (i: number) => `<!--yomi:end:${i}-->`;
export const MARKER = /^<!--yomi:(component|end):(\d+)-->\s*$/;

const NAME = "[A-Za-z][\\w.-]*";
const MDC_OPEN = new RegExp(`^(:{2,})(${NAME})(\\{.*\\})?\\s*$`);
const MDC_CLOSE = /^(:{2,})\s*$/;
const JSX_PAIRED = new RegExp(`^<(${NAME})(\\s[^>]*)?>(.*)</\\1>\\s*$`);
const JSX_SELF = new RegExp(`^<(${NAME})(\\s[^>]*?)?\\s*/>\\s*$`);
const JSX_OPEN = new RegExp(`^<(${NAME})(\\s[^>]*)?>\\s*$`);
const JSX_CLOSE = new RegExp(`^</(${NAME})>\\s*$`);
const COMMENT = /^<!--.*-->\s*$/;
const FENCE = /^(`{3,}|~{3,})/;
/** 行の途中の MDC の部品（`:badge[...]` や `:icon{...}`）。 */
const MDC_INLINE = /(^|[\s(])(:[A-Za-z][\w-]*)[[{]/;

/** 属性の並び（`type="warning" open` など）を読む。式（`{...}`）などは読めないとして null。 */
export function parseAttrs(raw: string): Record<string, AttrValue> | null {
  const out: Record<string, AttrValue> = {};
  const re = /\s*([A-Za-z_:][\w:.-]*)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'))?/y;
  let at = 0;
  const s = raw.trim();
  while (at < s.length) {
    re.lastIndex = at;
    const m = re.exec(s);
    if (!m || m[0].length === 0) return null;
    out[m[1]!] = m[2] ?? m[3] ?? true;
    at = re.lastIndex;
    while (s[at] === " " || s[at] === "\t") at++;
  }
  return out;
}

interface Open {
  index: number;
  kind: "mdc" | "jsx";
  /** MDC の場合はコロンの数、JSX の場合はタグ名。 */
  closer: string;
}

export function prepare(file: string, body: string, firstLine: number, componentMap: Record<string, string>): Prepared {
  const lines = body.split("\n");
  const out: string[] = [];
  const lineOf: number[] = [];
  const components: Component[] = [];
  const problems: string[] = [];
  const warnings: string[] = [];
  const stack: Open[] = [];
  let fence: string | null = null;

  const add = (name: string, rawAttrs: string | undefined, line: number): number | null => {
    const attrs = parseAttrs(rawAttrs ?? "");
    if (attrs === null) {
      problems.push(`${file}:${line}: component ${name} has attributes that cannot be imported (only name="value" and bare names)`);
      return null;
    }
    if (!Object.hasOwn(componentMap, name)) {
      problems.push(`${file}:${line}: component ${name} has no mapping in "componentMap"`);
    }
    components.push({ name, attrs, line });
    return components.length - 1;
  };

  const emit = (line: number, ...texts: string[]) => {
    for (const t of texts) {
      out.push(t);
      lineOf.push(line);
    }
  };
  lines.forEach((original, n) => {
    const line = firstLine + n;
    const text = original.trim();
    if (fence !== null) {
      if (text.startsWith(fence)) fence = null;
      emit(line, original);
      return;
    }
    const f = FENCE.exec(text);
    if (f) {
      fence = f[1]!;
      emit(line, original);
      return;
    }
    if (COMMENT.test(text)) {
      warnings.push(`${file}:${line}: an HTML comment was dropped`);
      emit(line, "");
      return;
    }
    let m: RegExpExecArray | null;
    if ((m = MDC_OPEN.exec(text))) {
      const attrs = m[3] ? m[3].slice(1, -1) : "";
      const i = add(m[2]!, attrs, line);
      if (i !== null) {
        stack.push({ index: i, kind: "mdc", closer: m[1]! });
        emit(line, "", openMarker(i), "");
      }
      return;
    }
    if ((m = MDC_CLOSE.exec(text))) {
      const top = stack.at(-1);
      if (top?.kind === "mdc" && top.closer === m[1]) {
        stack.pop();
        emit(line, "", closeMarker(top.index), "");
      } else {
        problems.push(`${file}:${line}: "${text}" does not close a component`);
      }
      return;
    }
    if ((m = JSX_PAIRED.exec(text))) {
      const i = add(m[1]!, m[2], line);
      if (i !== null) emit(line, "", openMarker(i), "", m[3]!, "", closeMarker(i), "");
      return;
    }
    if ((m = JSX_SELF.exec(text))) {
      const i = add(m[1]!, m[2], line);
      if (i !== null) emit(line, "", openMarker(i), "", closeMarker(i), "");
      return;
    }
    if ((m = JSX_OPEN.exec(text))) {
      const i = add(m[1]!, m[2], line);
      if (i !== null) {
        stack.push({ index: i, kind: "jsx", closer: m[1]! });
        emit(line, "", openMarker(i), "");
      }
      return;
    }
    if ((m = JSX_CLOSE.exec(text))) {
      const top = stack.at(-1);
      if (top?.kind === "jsx" && top.closer === m[1]) {
        stack.pop();
        emit(line, "", closeMarker(top.index), "");
      } else {
        problems.push(`${file}:${line}: </${m[1]}> does not close a component`);
      }
      return;
    }
    // 行の途中の MDC の部品（インラインのコードの中は除く）
    const inline = MDC_INLINE.exec(text.replace(/`[^`]*`/g, ""));
    if (inline) {
      problems.push(`${file}:${line}: inline component ${inline[2]} is not supported`);
    }
    emit(line, original);
  });
  for (const open of stack) {
    const c = components[open.index]!;
    problems.push(`${file}:${c.line}: component ${c.name} is not closed`);
  }
  return { text: out.join("\n"), lineOf, components, problems, warnings };
}
