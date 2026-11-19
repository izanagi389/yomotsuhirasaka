// 代替テキストの案を、本文の画像に当てる（「AI」タブの一覧から。2026-11-17）。
// 画像のノードの alt を変えるエディタの操作なので、元に戻せて、自動保存で下書きに入る。
import type { Editor } from "@tiptap/core";

/** 本文の中の、代替テキストも「装飾画像」の印もない画像（mediaId。重なりは1つにする）。 */
export function imagesMissingAlt(docs: readonly unknown[]): string[] {
  const out: string[] = [];
  const walk = (v: unknown) => {
    if (!v || typeof v !== "object") return;
    const node = v as { type?: unknown; attrs?: Record<string, unknown>; content?: unknown };
    if (node.type === "image") {
      const id = node.attrs?.["mediaId"];
      const alt = node.attrs?.["alt"];
      const has = (typeof alt === "string" && alt.trim() !== "") || node.attrs?.["decorative"] === true;
      if (typeof id === "string" && id && !has && !out.includes(id)) out.push(id);
    }
    if (Array.isArray(node.content)) node.content.forEach(walk);
  };
  docs.forEach(walk);
  return out;
}

/** その画像（代替テキストのないもの）に代替テキストを当てる。当てられなければ false。 */
export function applyAlt(editors: readonly Editor[], mediaId: string, alt: string): boolean {
  for (const editor of editors) {
    if (editor.isDestroyed || !editor.isEditable) continue;
    let at: number | null = null;
    editor.state.doc.descendants((node, pos) => {
      if (at !== null) return false;
      if (node.type.name === "image" && node.attrs["mediaId"] === mediaId && !String(node.attrs["alt"] ?? "").trim() && node.attrs["decorative"] !== true) {
        at = pos;
        return false;
      }
      return true;
    });
    if (at !== null) {
      const node = editor.state.doc.nodeAt(at)!;
      editor.view.dispatch(editor.state.tr.setNodeMarkup(at, undefined, { ...node.attrs, alt }));
      return true;
    }
  }
  return false;
}
