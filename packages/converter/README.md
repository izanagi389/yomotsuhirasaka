# @yomotsuhirasaka/converter

本文（Tiptap の JSON）を Markdown と HTML にする変換器です（`docs/decisions/03-content-and-editing.md`「本文」、Q56）。

- 標準のノードの変換は、ここに「最初から登録されている変換関数」として書きます。カスタムノードの変換関数は、テンプレートから渡します。
- サーバーは、ビルドしたものを組み込みJSで動かします。サーバーのビルドに Node を要らなくするため、ビルドしたものを `server/src/script/converter.js` にコピーしてコミットします。
- HTML の出力は、本文中の文字列をすべてエスケープします。カスタムノードの `toHtml` が返した HTML はそのまま入るので、必ず無害化してから使います（サーバーは ammonia で無害化します）。
- 出力が変わる変更をしたら `VERSION` を上げます（全記事の作り直しの判定に使います。Q58）。

```sh
npm run sync:server -w @yomotsuhirasaka/converter   # ビルドして、サーバーのコピーを更新する
npm test -w @yomotsuhirasaka/converter              # テスト（サーバーのコピーが最新かも確かめる）
```
