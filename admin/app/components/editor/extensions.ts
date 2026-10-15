// 管理画面のエディタの拡張機能。スキーマは共有パッケージ（@new-cms/editor-schema）のもので、ここでは画面の描き方だけを足す。
import { CmsEmbed, CmsImage, standardExtensions } from "@new-cms/editor-schema";
import { VueNodeViewRenderer } from "@tiptap/vue-3";

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
  return standardExtensions({ image, embed });
}
