# CMSのCLI

テンプレートのデプロイ、カスタムノードの雛形づくり、既存のブログ（Markdownのフォルダ）からの取り込みを行います。

使い方は手順書の [テンプレートとCLI](../docs/manual/developer/templates-and-cli.md) にあります。

リポジトリのルートの npm workspaces の一部です（管理画面と、本文のノードの定義 `@yomotsuhirasaka/editor-schema` を共有します）。

```sh
# リポジトリのルートで
npm ci && npm run build -w @yomotsuhirasaka/editor-schema && npm run build -w @yomotsuhirasaka/cli
```
