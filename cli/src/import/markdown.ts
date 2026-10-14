// Markdown の本文を、Tiptap の構造化JSONに変換する（markdown-it で解析する）。
//
// - ノードとマークの名前は Tiptap の標準（StarterKit と Link）と、CMSの画像（image）。
// - テンプレートの本文で使えないノードやマーク、表や生のHTMLがあれば、問題として返す（取り込みの前に止める）。
// - 画像は段落の外に出し、ブロックの画像にする。代替テキストのない画像は「飾り」として扱い、警告を出す。
// - 画像の mediaId は、アップロードの後に埋める（ここでは元のパスを覚えておく）。

import MarkdownIt from "markdown-it";
import type { Token } from "markdown-it";
import { MARKER, type Component } from "./components.js";

export interface TiptapNode {
  type: string;
  attrs?: Record<string, unknown>;
  content?: TiptapNode[];
  marks?: { type: string; attrs?: Record<string, unknown> }[];
  text?: string;
}

export interface ImageRef {
  /** 書いてあったとおりのパス。 */
  src: string;
  line: number;
  node: TiptapNode;
}

export interface BodyRules {
  nodes: Set<string>;
  marks: Set<string>;
}

export interface Converted {
  doc: TiptapNode;
  images: ImageRef[];
  problems: string[];
  warnings: string[];
}

const md = new MarkdownIt("default", { html: true, linkify: false, typographer: false });

type Mark = NonNullable<TiptapNode["marks"]>[number];

const CJK = /[　-ヿ㐀-鿿豈-﫿＀-￯]/;

export function convert(
  file: string,
  text: string,
  lineOf: number[],
  components: Component[],
  componentMap: Record<string, string>,
  rules: BodyRules,
): Converted {
  const problems: string[] = [];
  const warnings: string[] = [];
  const images: ImageRef[] = [];
  const usedNodes = new Map<string, number>();
  const usedMarks = new Map<string, number>();
  const tokens = md.parse(text, {});
  const at = (t: Token | undefined): number => {
    const l = t?.map?.[0];
    return l === undefined ? (lineOf[0] ?? 1) : (lineOf[l] ?? lineOf.at(-1) ?? 1);
  };
  const node = (type: string, line: number, extra: Omit<TiptapNode, "type"> = {}): TiptapNode => {
    if (!usedNodes.has(type)) usedNodes.set(type, line);
    return { type, ...extra };
  };

  // --- インライン ---
  function inline(children: Token[], line: number): TiptapNode[] {
    const out: TiptapNode[] = [];
    const marks: Mark[] = [];
    const push = (n: TiptapNode) => {
      const last = out.at(-1);
      if (n.type === "text" && last?.type === "text" && JSON.stringify(last.marks ?? []) === JSON.stringify(n.marks ?? [])) {
        last.text = (last.text ?? "") + n.text;
      } else {
        out.push(n);
      }
    };
    const textNode = (t: string, extra: Mark[] = []) => {
      if (t.length === 0) return;
      const all = [...marks, ...extra];
      for (const m of all) if (!usedMarks.has(m.type)) usedMarks.set(m.type, line);
      push(all.length > 0 ? { type: "text", text: t, marks: all.map((m) => ({ ...m })) } : { type: "text", text: t });
    };
    const pop = (type: string) => {
      const i = marks.map((m) => m.type).lastIndexOf(type);
      if (i >= 0) marks.splice(i, 1);
    };
    for (let i = 0; i < children.length; i++) {
      const t = children[i]!;
      switch (t.type) {
        case "text":
          textNode(t.content);
          break;
        case "softbreak": {
          // 改行は空白にする。日本語どうしの間では詰める
          const prev = children[i - 1]?.content.at(-1) ?? "";
          const next = children[i + 1]?.content[0] ?? "";
          if (!(CJK.test(prev) && CJK.test(next))) textNode(" ");
          break;
        }
        case "hardbreak":
          out.push(node("hardBreak", line));
          break;
        case "strong_open":
          marks.push({ type: "bold" });
          break;
        case "strong_close":
          pop("bold");
          break;
        case "em_open":
          marks.push({ type: "italic" });
          break;
        case "em_close":
          pop("italic");
          break;
        case "s_open":
          marks.push({ type: "strike" });
          break;
        case "s_close":
          pop("strike");
          break;
        case "code_inline":
          textNode(t.content, [{ type: "code" }]);
          break;
        case "link_open":
          marks.push({ type: "link", attrs: { href: String(t.attrGet("href") ?? "") } });
          break;
        case "link_close":
          pop("link");
          break;
        case "image": {
          const alt = t.content.trim();
          const attrs: Record<string, unknown> = alt ? { mediaId: "", alt } : { mediaId: "", decorative: true };
          if (!alt) warnings.push(`${file}:${line}: an image without alternative text was imported as decorative`);
          const n = node("image", line, { attrs });
          images.push({ src: String(t.attrGet("src") ?? ""), line, node: n });
          out.push(n);
          break;
        }
        case "html_inline":
          problems.push(`${file}:${line}: inline HTML or components (${t.content.trim()}) are not supported`);
          break;
        default:
          problems.push(`${file}:${line}: unsupported inline markup (${t.type})`);
      }
    }
    return out;
  }

  /** 段落の中の画像を外に出す（画像はブロックのノード）。空の段落は作らない。 */
  function paragraphs(inlines: TiptapNode[], line: number): TiptapNode[] {
    const out: TiptapNode[] = [];
    let run: TiptapNode[] = [];
    const flush = () => {
      while (run[0]?.type === "text" && run[0].text?.trim() === "" ) run.shift();
      while (run.at(-1)?.type === "text" && run.at(-1)!.text?.trim() === "") run.pop();
      if (run.length > 0) out.push(node("paragraph", line, { content: run }));
      run = [];
    };
    for (const n of inlines) {
      if (n.type === "image") {
        flush();
        out.push(n);
      } else {
        run.push(n);
      }
    }
    flush();
    return out;
  }

  // --- ブロック ---
  /** `end` の種類の閉じのトークンまで（または印の終わりまで）を変換する。 */
  function blocks(i: number, end: string | null, endMarker: number | null): [TiptapNode[], number] {
    const out: TiptapNode[] = [];
    while (i < tokens.length) {
      const t = tokens[i]!;
      const line = at(t);
      if (end !== null && t.type === end) return [out, i + 1];
      switch (t.type) {
        case "paragraph_open": {
          const content = tokens[i + 1]!;
          out.push(...paragraphs(inline(content.children ?? [], line), line));
          i += 3;
          continue;
        }
        case "heading_open": {
          const content = tokens[i + 1]!;
          const level = Number(t.tag.slice(1));
          const kids = inline(content.children ?? [], line);
          const textual = kids.filter((k) => k.type !== "image");
          if (kids.length !== textual.length) problems.push(`${file}:${line}: images in headings are not supported`);
          out.push(node("heading", line, { attrs: { level }, content: textual }));
          i += 3;
          continue;
        }
        case "bullet_list_open":
        case "ordered_list_open": {
          const ordered = t.type === "ordered_list_open";
          const [items, next] = blocks(i + 1, ordered ? "ordered_list_close" : "bullet_list_close", null);
          const start = Number(t.attrGet("start") ?? "1");
          out.push(node(ordered ? "orderedList" : "bulletList", line, ordered && start !== 1 ? { attrs: { start }, content: items } : { content: items }));
          i = next;
          continue;
        }
        case "list_item_open": {
          const [content, next] = blocks(i + 1, "list_item_close", null);
          out.push(node("listItem", line, { content }));
          i = next;
          continue;
        }
        case "blockquote_open": {
          const [content, next] = blocks(i + 1, "blockquote_close", null);
          out.push(node("blockquote", line, { content }));
          i = next;
          continue;
        }
        case "fence":
        case "code_block": {
          const code = t.content.replace(/\n$/, "");
          const language = t.type === "fence" ? t.info.trim().split(/\s+/)[0] || null : null;
          out.push(node("codeBlock", line, { attrs: { language }, ...(code ? { content: [{ type: "text", text: code }] } : {}) }));
          i += 1;
          continue;
        }
        case "hr":
          out.push(node("horizontalRule", line));
          i += 1;
          continue;
        case "html_block": {
          const m = MARKER.exec(t.content.trim());
          if (m && m[1] === "component") {
            const index = Number(m[2]);
            const c = components[index]!;
            const [content, next] = blocks(i + 1, null, index);
            const mapped = componentMap[c.name];
            if (mapped) out.push(node(mapped, c.line, { ...(Object.keys(c.attrs).length > 0 ? { attrs: c.attrs } : {}), ...(content.length > 0 ? { content } : {}) }));
            i = next;
            continue;
          }
          if (m && m[1] === "end") {
            if (endMarker === Number(m[2])) return [out, i + 1];
            problems.push(`${file}:${line}: a component is closed in the wrong place`);
            i += 1;
            continue;
          }
          problems.push(`${file}:${line}: raw HTML is not supported (${t.content.trim().split("\n")[0]})`);
          i += 1;
          continue;
        }
        case "table_open": {
          problems.push(`${file}:${line}: tables are not supported`);
          while (i < tokens.length && tokens[i]!.type !== "table_close") i++;
          i += 1;
          continue;
        }
        default:
          problems.push(`${file}:${line}: unsupported markup (${t.type})`);
          i += 1;
      }
    }
    if (endMarker !== null) problems.push(`${file}: a component is not closed`);
    return [out, i];
  }

  const [content] = blocks(0, null, null);
  for (const [name, line] of usedNodes) {
    if (!rules.nodes.has(name)) problems.push(`${file}:${line}: the template's body does not allow "${name}"`);
  }
  for (const [name, line] of usedMarks) {
    if (!rules.marks.has(name)) problems.push(`${file}:${line}: the template's body does not allow the "${name}" mark`);
  }
  return { doc: { type: "doc", content }, images, problems, warnings };
}
