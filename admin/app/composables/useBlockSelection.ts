// いま触っている本文のエディタ（右の欄の「ブロック」タブが、そのエディタの選んだブロックを設定する）。
// 本文の項目が複数あっても、最後にカーソルを置いたものが対象になる。Editor は大きなオブジェクトなので、
// 深いリアクティブにはせず、変化は tick で知らせる。
import type { Editor } from "@tiptap/core";

const active = shallowRef<Editor | null>(null);
const tick = ref(0);

export function useBlockSelection() {
  return { active, tick };
}

/** エディタを登録する。戻り値で外す（エディタを壊す前に呼ぶ）。 */
export function trackBlockSelection(editor: Editor): () => void {
  const take = () => {
    active.value = editor;
    tick.value++;
  };
  const touch = () => {
    if (active.value === editor) tick.value++;
  };
  editor.on("focus", take);
  editor.on("selectionUpdate", take);
  editor.on("transaction", touch);
  return () => {
    editor.off("focus", take);
    editor.off("selectionUpdate", take);
    editor.off("transaction", touch);
    if (active.value === editor) active.value = null;
  };
}
