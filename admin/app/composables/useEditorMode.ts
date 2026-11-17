// 本文のエディタの種類（ブロック型・クラシック。03-content-and-editing.md「本文のエディタ」）。
// 選んだものはこのブラウザにだけ覚える（サーバーには送らない）。最初はブロック型。覚えられない環境でもブロック型。
// どちらも同じ Tiptap の JSON を編集するので、切り替えても本文は変わらない。

export type EditorMode = "block" | "classic";
export const EDITOR_MODES: readonly EditorMode[] = ["block", "classic"];
const KEY = "yomi:editor";

function stored(): EditorMode {
  try {
    return localStorage.getItem(KEY) === "classic" ? "classic" : "block";
  } catch {
    return "block";
  }
}

export function useEditorMode() {
  const mode = useState<EditorMode>("editorMode", () => (import.meta.client ? stored() : "block"));

  function setMode(m: EditorMode) {
    mode.value = m;
    try {
      if (m === "block") localStorage.removeItem(KEY);
      else localStorage.setItem(KEY, m);
    } catch {
      // 覚えられなくても、この画面には反映する
    }
  }

  return { mode, setMode };
}
