// CMSの本文の標準の拡張機能の一覧（Q57: ノードの定義はTiptapの拡張機能が正）。
// 管理画面のエディタと、マニフェストを作るCLIの両方が、この一覧を使う。

import type { AnyExtension } from "@tiptap/core";
import { StarterKit } from "@tiptap/starter-kit";

import { YomiEmbed, YomiImage, HeadingAnchors } from "./nodes.ts";

export interface StandardOptions {
  /** 画像のノード（管理画面は、ノードビューを足した `YomiImage.extend(...)` を渡す）。 */
  image?: AnyExtension;
  /** 埋め込みのノード。 */
  embed?: AnyExtension;
}

/**
 * 標準の拡張機能。
 * - StarterKit から下線（underline）を除く（Markdownに対応がなく、見た目の指定のため）。
 * - リンクには target・rel を付けない（どう開くかはサイト側が決める。原則1）。
 */
export function standardExtensions(options: StandardOptions = {}): AnyExtension[] {
  return [
    StarterKit.configure({
      underline: false,
      link: {
        openOnClick: false,
        autolink: true,
        HTMLAttributes: { target: null, rel: null, class: null },
      },
    }),
    HeadingAnchors,
    options.image ?? YomiImage,
    options.embed ?? YomiEmbed,
  ];
}
