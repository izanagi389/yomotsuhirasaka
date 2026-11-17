// 校正の提案を、編集中の本文に当てる（10-llm-and-budget.md「編集者向けのAI支援」の5、2026-11-17）。
// 当てられるのは、元の語句が開いている本文のエディタのすべてを合わせてちょうど1か所だけあり、その1か所が1つの段落の中の、
// 書式（リンク・太字など）のないひと続きの文字に収まるときだけ（サーバーの assist::proofread::applicable と同じ規則。原則4）。
// コードのブロックの中は探さない。置き換えはエディタの操作なので、元に戻せて、自動保存で下書きに入る。
import type { Editor } from "@tiptap/core";

/** 段落の中の、文字でないノード（画像・改行など）の代わりの字。元の語句がこれをまたがないようにする。 */
const OBJECT = "￼";

interface Hit {
  editor: Editor;
  from: number;
  to: number;
}

/** 元の語句の、ただ1つの当てられる位置（なければ null）。 */
export function locateFix(editors: readonly Editor[], text: string): Hit | null {
  if (!text || text.includes(OBJECT)) return null;
  let count = 0;
  let hit: Hit | null = null;
  for (const editor of editors) {
    if (editor.isDestroyed) continue;
    editor.state.doc.descendants((node, pos) => {
      if (count > 1 || node.type.name === "codeBlock") return false;
      if (!node.isTextblock) return true;
      let s = "";
      const segments: { start: number; end: number; pos: number; plain: boolean }[] = [];
      node.forEach((child, offset) => {
        const start = s.length;
        if (child.isText) {
          s += child.text ?? "";
          segments.push({ start, end: s.length, pos: pos + 1 + offset, plain: child.marks.length === 0 });
        } else {
          s += OBJECT;
        }
      });
      // 重なり合う出現も数える
      for (let i = s.indexOf(text); i !== -1; i = s.indexOf(text, i + 1)) {
        count++;
        const seg = segments.find((g) => g.plain && g.start <= i && i + text.length <= g.end);
        hit = seg ? { editor, from: seg.pos + (i - seg.start), to: seg.pos + (i - seg.start) + text.length } : null;
      }
      return false;
    });
  }
  return count === 1 ? hit : null;
}

/** 提案を当てる。当てられなければ false（本文は変えない）。 */
export function applyFix(editors: readonly Editor[], from: string, to: string): boolean {
  const hit = locateFix(editors, from);
  if (!hit || !hit.editor.isEditable) return false;
  hit.editor.view.dispatch(hit.editor.state.tr.insertText(to, hit.from, hit.to));
  return true;
}
