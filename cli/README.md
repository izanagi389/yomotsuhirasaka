# CMSのCLI

テンプレートのデプロイ、カスタムノードの雛形づくり、既存のブログ（Markdownのフォルダ）からの取り込みを行います。

リポジトリのルートの npm workspaces の一部です（管理画面と、本文のノードの定義 `@yomotsuhirasaka/editor-schema` を共有します）。

```sh
# リポジトリのルートで
npm ci && npm run build -w @yomotsuhirasaka/editor-schema && npm run build -w @yomotsuhirasaka/cli
```

## テンプレートのデプロイ（`docs/decisions/02-schema-and-delivery-api.md`）

テンプレートは `templates/<テンプレート名>/template.json` に書きます（例: `templates/blog/template.json`）。フォルダの名前がテンプレートの名前です。

- `$schema` に `cli/schema/template.schema.json` を指定すると、エディタで補完と検証が効きます（サーバーのマニフェストの型から作ったもの）。
- 本文のノードとマークは書きません。CLIが、管理画面と同じTiptapの拡張機能から書き出します（Q57）。埋め込みの提供元（`body.embedProviders`）は書きます。

```sh
# デプロイの範囲の個人トークン（サーバー管理者・多要素認証）
YOMI_TOKEN=<個人トークン> node cli/dist/index.js deploy templates/blog --url https://yomi.example.com [--dry-run]
node cli/dist/index.js schema build templates/blog   # マニフェストを表示するだけ
```

- まずサーバーで試行し（何も書き込まない）、マニフェストの問題・破壊的な変更・壊れる登録済みのクエリを表示します。判定はサーバーが行い、デプロイのときにもう一度検査します。
- マニフェストに問題があれば止めます。破壊的な変更は `--allow-destructive`、登録済みのクエリを壊す変更は `--allow-broken-queries` がなければ止めます（Q53, Q54）。
- `--dry-run` は試行だけで終わります。

### 関数（URL生成関数・フック・カスタムノードの変換関数）

関数は、テンプレートのフォルダの中の TypeScript のファイルに書き、`template.json` からパスで参照します（Q49）。

```json
{
  "body": {
    "customNodes": [
      { "name": "callout", "attrs": [{ "name": "label", "type": "string" }], "toMarkdown": { "file": "./nodes/callout.ts" } }
    ]
  },
  "contentTypes": [
    {
      "name": "post",
      "urlFunction": { "file": "./functions/url.ts", "permissions": ["content.read"] },
      "hooks": { "afterPublish": { "file": "./functions/notify.ts", "permissions": ["http.fetch"], "allowedHosts": ["hooks.example.com"] } }
    }
  ]
}
```

- CLIが esbuild で1つの ES モジュールにまとめ、マニフェストに JS のソースとして入れます。サーバーはソースだけを受け取ります。
- `import` できるのは、テンプレートのフォルダの中の相対パスだけです。npm のパッケージや `node:` は使えません。型は `import type { ... } from "@yomotsuhirasaka/cli/types"` で使えます（変換で消えます）。
- フォルダの外を指すパス（`..`、絶対パス、外に出るシンボリックリンク）は止めます。
- デプロイの前に、サーバーと同じ規則で早めに検査します（カスタムノードの `toMarkdown` は必須、`http.fetch` は非同期のフックだけ、など）。判定はサーバーがもう一度行います。

カスタムノードの変換関数の雛形は、次で作ります。`template.json` は書き換えず、足す宣言を表示します。

```sh
node cli/dist/index.js node scaffold callout --template templates/blog [--inline]
```
- 終了コード: 0 成功、1 デプロイしなかった・移行待ちのワークスペースがある、2 使い方の誤り。

## 既存のブログからの取り込み（`docs/decisions/03-content-and-editing.md`「既存ブログからの取り込み」）

```sh
YOMI_TOKEN=<個人トークン（write の範囲）> node cli/dist/index.js import ./content \
  --url https://yomi.example.com --workspace <ワークスペースID> \
  --config import.json --redirects redirects.json [--static-dir ./public]
```

- すべてのファイルを先に検査し、1つでも問題があれば何も取り込みません。
- 取り込んだ記事は公開されます。元のURLは、新しいURLとの対応としてURLの変更履歴に残ります。
- `redirects.json` には、Nuxt の `routeRules`（元のURLのパス → 新しいURLのパス、301）を書き出します。
- 代替テキストのない画像は「飾り」として取り込み、警告を出します。
- 画像はフォルダの中のファイルだけを読みます（`/` で始まるパスは `--static-dir` の中）。外部のURLの画像は取り込めません。

設定ファイル（JSON、すべて省略可）:

| キー | 既定 | 内容 |
|---|---|---|
| `contentType` | コンテンツタイプが1つならそれ | 取り込み先のコンテンツタイプ |
| `originalUrlKey` | `url` | 元のURLを持つ frontmatter のキー（値は必須） |
| `localeKey` | `locale` | ロケールのキー（なければ既定のロケール） |
| `translationKey` | なし | 翻訳グループにまとめるキー |
| `translationOf` | なし | 元の記事のスラッグを指すキー |
| `bodyField` | 本文のフィールドが1つならそれ | 本文を入れるフィールド |
| `componentMap` | `{}` | 独自部品（`<Callout>`・`::callout`）→ 本文のノード。対応のない部品があれば止める |
| `ignoreKeys` | `[]` | 取り込まない frontmatter のキー。フィールドにないキーは、ここに書かないと止める |
