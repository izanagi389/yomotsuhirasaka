# @yomotsuhirasaka/editor-schema

本文のノードの定義（Tiptapの拡張機能）です。`docs/decisions/02-schema-and-delivery-api.md`（Q57, Q142）のとおり、ノードの定義は Tiptap の拡張機能を正とし、次の2か所が同じものを使います。

- 管理画面のエディタ（`admin/`）: `standardExtensions()` に画面の描き方（Vue のノードビュー）を足して使う。ワークスペースのマニフェストにあるノードとマークだけに絞る（`forManifest`）
- マニフェストを作るCLI（これから作る）: `bodyDefinition(standardExtensions())` で、マニフェストの `body` のノードとマークを書き出す

## 標準の拡張機能

- Tiptap の StarterKit（下線を除く。リンクには `target`・`rel` を付けない）
- 見出しのアンカーID（`HeadingAnchors`）: 仮のIDを振る。最終的にはサーバーが保存時に決める（Q28, Q136）
- 画像（`image`: `mediaId`・`alt`・`decorative`）と埋め込み（`embed`: `src`）

## ビルド

管理画面とCLIは、ビルドした `dist/` を使います（CLIは Node でそのまま動かすため）。

```sh
npm run build -w @yomotsuhirasaka/editor-schema
```

## テスト

```sh
npm test -w @yomotsuhirasaka/editor-schema
npm run typecheck -w @yomotsuhirasaka/editor-schema
```
