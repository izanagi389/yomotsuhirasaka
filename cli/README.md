# CMSのCLI

M1では、既存のブログ（Markdownのフォルダ）からの取り込みだけを行います（`docs/decisions/03-content-and-editing.md`「既存ブログからの取り込み」）。

```sh
npm ci && npm run build
CMS_TOKEN=<個人トークン（write の範囲）> node dist/index.js import ./content \
  --url https://cms.example.com --workspace <ワークスペースID> \
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
