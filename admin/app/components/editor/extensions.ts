// 管理画面のエディタの拡張機能。スキーマは共有パッケージ（@new-cms/editor-schema）のもので、ここでは画面の描き方だけを足す。
import { CmsEmbed, CmsImage, type CustomNodeDecl, customNodeExtension, standardExtensions } from "@new-cms/editor-schema";
import { VueNodeViewRenderer } from "@tiptap/vue-3";
// StarterKit のコマンド（toggleBold など）の型を読み込む（共有パッケージの型定義には含まれないため）
import type {} from "@tiptap/starter-kit";

import CustomNodeView from "./CustomNodeView.vue";
import EmbedView from "./EmbedView.vue";
import ImageView from "./ImageView.vue";

export interface ViewOptions {
  /** 画像のメディアを読むワークスペース。 */
  workspaceId: string;
  /** 埋め込みを許す提供元（マニフェストの embedProviders）。 */
  embedProviders: string[];
  /** 編集できるか（ノードビューの入力欄を、エディタと一緒に止めるため）。リアクティブな値を読む関数で渡す
   *（Tiptap は拡張機能のオプションを写すので、ref をそのまま渡すと追従しない）。 */
  isEditable: () => boolean;
  /** カスタムノードの宣言（マニフェストの body.customNodes）。 */
  customNodes: CustomNodeDecl[];
}

export function editorExtensions(options: ViewOptions) {
  const image = CmsImage.extend({
    addOptions: () => ({ workspaceId: options.workspaceId, isEditable: options.isEditable }),
    addNodeView: () => VueNodeViewRenderer(ImageView),
  });
  const embed = CmsEmbed.extend({
    addOptions: () => ({ embedProviders: options.embedProviders }),
    addNodeView: () => VueNodeViewRenderer(EmbedView),
  });
  const customs = options.customNodes.map((decl) =>
    customNodeExtension(decl).extend({
      addOptions: () => ({ decl, isEditable: options.isEditable }),
      // カードの中の入力欄やスイッチの操作は、エディタ（ProseMirror）に渡さない（渡すとノード全体が選ばれ、
      // 次の挿入でノードが置き換えられてしまう）。子の編集欄（NodeViewContent）の操作は渡す
      addNodeView: () =>
        VueNodeViewRenderer(CustomNodeView, {
          stopEvent: ({ event }) => event.target instanceof Element && event.target.closest("[data-custom-form]") !== null,
        }),
    }),
  );
  return [...standardExtensions({ image, embed }), ...customs];
}
