// ブロック型のエディタの、ブロックの操作（並べ替え・複製・削除）と、「/」の候補の拡張機能。
// - ブロックは本文（doc）の直下のノード。リストや引用は、中身ごと1つのブロックとして扱う
// - 並べ替えは、隣のブロックと入れ替える（Alt+Shift+↑↓ でも動かせる）
// - 「/」は行の頭（本文の直下の段落）でだけ開く。候補の一覧と絞り込みは RichTextEditor が描く
import { Extension, type Editor, type Range } from "@tiptap/core";
import { Fragment, type Node as PmNode } from "@tiptap/pm/model";
import { NodeSelection, PluginKey, Selection, type EditorState } from "@tiptap/pm/state";
import Suggestion, { type SuggestionKeyDownProps, type SuggestionProps } from "@tiptap/suggestion";

export interface Block {
  /** 本文の直下での番号。 */
  index: number;
  /** ブロックの始まりの位置。 */
  pos: number;
  node: PmNode;
}

/** 番号のブロック。 */
export function blockAt(doc: PmNode, index: number): Block | null {
  if (index < 0 || index >= doc.childCount) return null;
  let pos = 0;
  for (let i = 0; i < index; i++) pos += doc.child(i).nodeSize;
  return { index, pos, node: doc.child(index) };
}

/** カーソル（または選んだノード）のあるブロック。 */
export function selectedBlock(state: EditorState): Block | null {
  const { selection, doc } = state;
  if (selection instanceof NodeSelection && selection.$from.depth === 0) return blockAt(doc, selection.$from.index(0));
  if (selection.$from.depth === 0) return null;
  return blockAt(doc, selection.$from.index(0));
}

/** ブロックを1つ上（-1）か下（+1）へ動かす。動かせなければ false。 */
export function moveBlock(editor: Editor, index: number, dir: -1 | 1): boolean {
  const { state, view } = editor;
  const upper = blockAt(state.doc, dir < 0 ? index - 1 : index);
  const lower = blockAt(state.doc, dir < 0 ? index : index + 1);
  if (!upper || !lower) return false;
  const tr = state.tr.replaceWith(upper.pos, lower.pos + lower.node.nodeSize, Fragment.from([lower.node, upper.node]));
  // 動かしたブロックの頭にカーソルを置く
  const movedPos = dir < 0 ? upper.pos : upper.pos + lower.node.nodeSize;
  tr.setSelection(Selection.near(tr.doc.resolve(movedPos + (tr.doc.nodeAt(movedPos)?.isAtom ? 0 : 1))));
  view.dispatch(tr.scrollIntoView());
  view.focus();
  return true;
}

/** ブロックの写しを、すぐ後ろに入れる。 */
export function duplicateBlock(editor: Editor, index: number): void {
  const { state, view } = editor;
  const b = blockAt(state.doc, index);
  if (!b) return;
  view.dispatch(state.tr.insert(b.pos + b.node.nodeSize, b.node.copy(b.node.content)).scrollIntoView());
  view.focus();
}

/** ブロックを消す（最後の1つなら、空の段落が残る）。 */
export function deleteBlock(editor: Editor, index: number): void {
  const { state, view } = editor;
  const b = blockAt(state.doc, index);
  if (!b) return;
  view.dispatch(state.tr.delete(b.pos, b.pos + b.node.nodeSize).scrollIntoView());
  view.focus();
}

/** ブロックを選んだ状態にして、ドラッグを始める（ProseMirror のノードのドラッグをそのまま使う。落とす先の線は Dropcursor が出す）。 */
export function startBlockDrag(editor: Editor, index: number, event: DragEvent, image: Element | null): void {
  const { view } = editor;
  const b = blockAt(view.state.doc, index);
  if (!b || !event.dataTransfer) return;
  const selection = NodeSelection.create(view.state.doc, b.pos);
  view.dispatch(view.state.tr.setSelection(selection));
  const slice = selection.content();
  event.dataTransfer.effectAllowed = "move";
  event.dataTransfer.setData("text/plain", b.node.textContent);
  if (image) event.dataTransfer.setDragImage(image, 0, 0);
  view.dragging = { slice, move: true };
}

/** ドラッグが本文の外で終わったときの後片付け（本文の中に落とせば、ProseMirror が片付ける）。 */
export function endBlockDrag(editor: Editor): void {
  editor.view.dragging = null;
}

export interface SlashHandlers {
  /** 「/」を使えるか（ブロック型で、編集できるとき）。 */
  enabled: () => boolean;
  open: (query: string, rect: DOMRect | null, range: Range) => void;
  close: () => void;
  /** 一覧の上下・決定・閉じる。扱ったら true。 */
  key: (event: KeyboardEvent, range: Range) => boolean;
}

/** 行の頭の「/」で、ブロックの候補を開く拡張機能。 */
export function slashCommand(handlers: SlashHandlers) {
  return Extension.create({
    name: "blockSlash",
    addProseMirrorPlugins() {
      return [
        Suggestion({
          pluginKey: new PluginKey("blockSlash"),
          editor: this.editor,
          char: "/",
          startOfLine: true,
          allow: ({ state, range }) => {
            const $from = state.doc.resolve(range.from);
            return handlers.enabled() && $from.depth === 1 && $from.parent.type.name === "paragraph";
          },
          render: () => ({
            onStart: (p: SuggestionProps) => handlers.open(p.query, p.clientRect?.() ?? null, p.range),
            onUpdate: (p: SuggestionProps) => handlers.open(p.query, p.clientRect?.() ?? null, p.range),
            onExit: () => handlers.close(),
            onKeyDown: (p: SuggestionKeyDownProps) => handlers.key(p.event, p.range),
          }),
        }),
      ];
    },
  });
}

/** Alt+Shift+↑↓ でブロックを動かす拡張機能（キーボードだけでも並べ替えられるように）。 */
export const blockKeys = Extension.create({
  name: "blockKeys",
  addKeyboardShortcuts() {
    const move = (dir: -1 | 1) => () => {
      const b = selectedBlock(this.editor.state);
      return b ? moveBlock(this.editor, b.index, dir) : false;
    };
    return { "Alt-Shift-ArrowUp": move(-1), "Alt-Shift-ArrowDown": move(1) };
  },
});

/** 右の欄の「ブロック」タブで設定するノード（選んだノード。なければカーソルのある見出しかカスタム部品。なければ本文の直下のブロック）。 */
export interface BlockTarget {
  pos: number;
  node: PmNode;
}
export function blockTarget(state: EditorState, isCustom: (name: string) => boolean): BlockTarget | null {
  const { selection } = state;
  if (selection instanceof NodeSelection) return { pos: selection.from, node: selection.node };
  const $from = selection.$from;
  for (let d = $from.depth; d > 0; d--) {
    const n = $from.node(d);
    if (n.type.name === "heading" || isCustom(n.type.name)) return { pos: $from.before(d), node: n };
  }
  return $from.depth >= 1 ? { pos: $from.before(1), node: $from.node(1) } : null;
}

/** ノードの属性を変える（ほかの属性はそのまま）。 */
export function setBlockAttrs(editor: Editor, target: BlockTarget, attrs: Record<string, unknown>): void {
  const { state, view } = editor;
  const node = state.doc.nodeAt(target.pos);
  if (!node || node.type !== target.node.type) return;
  view.dispatch(state.tr.setNodeMarkup(target.pos, undefined, { ...node.attrs, ...attrs }));
}
