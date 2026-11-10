# テンプレートとCLI

CLI の入れ方は [最初のサイトを作るまでの流れ](getting-started.md#1-cli-とモジュールをソースからビルドする) にあります。

## コマンドの一覧

| コマンド | 内容 | トークン |
|---|---|---|
| `yomi deploy <フォルダ> --url <CMSのURL>` | テンプレートをデプロイする | デプロイの範囲 |
| `yomi schema build <フォルダ> [--out <ファイル>]` | マニフェストを書き出すだけ | 不要 |
| `yomi node scaffold <名前> --template <フォルダ> [--inline]` | カスタムノードの雛形を作る | 不要 |
| `yomi import <フォルダ> --url <CMSのURL> --workspace <ID> --redirects <出力>` | 既存のブログを取り込む | 読むと書くの範囲 |

トークンは環境変数 `YOMI_TOKEN` で渡します（引数に書くと、プロセスの一覧に出るためです）。トークンは管理画面の「アカウント」→「個人トークン」で発行します。

終了コード: 0 成功、1 デプロイしなかった・移行待ちのワークスペースがある・取り込まなかった、2 使い方の誤り、3 予期しないエラー。

## テンプレートの書き方

テンプレートは `templates/<テンプレート名>/template.json` に書きます。フォルダの名前がテンプレートの名前です。`$schema` に `cli/schema/template.schema.json` を指定すると、エディタで補完と検証が効きます。

```json
{
  "$schema": "../../cli/schema/template.schema.json",
  "locales": ["ja", "en"],
  "defaultLocale": "ja",
  "body": { "embedProviders": ["www.youtube.com"] },
  "contentTypes": [
    {
      "name": "post",
      "urlTemplate": "https://example.com/{locale}/blog/{slug}",
      "titleField": "title",
      "listLimit": 50,
      "review": { "intervalDays": 365 },
      "aiMarkdown": { "title": "title", "meta": ["summary"], "fields": ["body"], "relations": ["related"] },
      "fields": [
        { "name": "title", "type": "text", "required": true, "searchable": true, "description": "記事のタイトル" },
        { "name": "slug", "type": "slug", "required": true },
        { "name": "summary", "type": "text" },
        { "name": "body", "type": "richtext", "required": true, "searchable": true },
        { "name": "related", "type": "reference", "referenceTo": "post", "multiple": true }
      ]
    }
  ]
}
```

### 全体

| 項目 | 内容 |
|---|---|
| `locales` | コンテンツのロケール |
| `defaultLocale` | 既定のロケール（`locales` のどれか） |
| `body.embedProviders` | 埋め込みを許す提供元のホスト名。https の URL だけを受け付けます |
| `body.customNodes` | カスタムノード（[関数](functions.md#カスタムノード)） |
| `contentTypes` | コンテンツタイプ |

本文の標準のノードとマークは書きません。CLI が、管理画面と同じ Tiptap の拡張機能から書き出します。

### コンテンツタイプ

| 項目 | 内容 |
|---|---|
| `name` | 英小文字で始まる英小文字、数字、`_`。DB と GraphQL の名前の元になります |
| `urlTemplate` | 公開 URL のテンプレート。使える差し込みは `{locale}` と `{slug}` |
| `urlFunction` | テンプレートで書けない URL を作る関数（あれば `urlTemplate` の代わりに使います） |
| `titleField` | 記事のタイトルのフィールド（text）。訂正フィードや一覧で記事の名前に使います |
| `hooks` | フック（[関数](functions.md)） |
| `review.intervalDays` | 確認期限の既定（1〜3650日）。なければ期限なし。記事ごとに変えられます |
| `aiMarkdown` | AI 向けの Markdown 版に載せるもの。挙げていないフィールドは載せません。なければ、このタイプの記事は AI 向けの素材になりません |
| `listLimit` | 一覧で1回に取得できる件数の上限 |
| `description` | 編集者向けの説明（管理画面に出ます） |
| `devNote` | 開発者向けのメモ。**どの API にも出ません** |

### フィールド

| 項目 | 内容 |
|---|---|
| `name` | 英小文字で始まる英小文字、数字、`_` |
| `type` | `text`（1行）、`slug`（英小文字、数字、ハイフン）、`richtext`（本文）、`reference`（他の記事への参照） |
| `required` | 公開に必須か（スラッグは常に必須） |
| `referenceTo`、`multiple` | 参照先のコンテンツタイプ、複数の値を持つか（参照フィールドだけ） |
| `searchable` | 検索と質問応答の対象か。**既定は対象外**です。使えるのは `text` と `richtext` だけです |
| `description`、`devNote` | コンテンツタイプと同じ |

多言語は記事ごとです。ロケールごとに、すべてのフィールドを持つ独立した記事を作り、翻訳グループで結びます。参照フィールドは翻訳グループを指し、配信のときに同じロケールの公開中の記事に解決します（なければ返しません）。

## デプロイ

デプロイできるのは、多要素認証を設定したサーバー管理者だけです。**マニフェストに JS が入るので、デプロイの権限はサーバーの上でコードを動かす権限と同じです。** そのため、デプロイの範囲のトークンはほかの範囲と混ぜられず、有効期限は最長30日です。CI で使うときは、期限の前に発行し直してください。

```bash
YOMI_TOKEN=<デプロイの範囲の個人トークン> yomi deploy templates/blog --url https://yomi.example.com --dry-run
```

```bash
YOMI_TOKEN=<デプロイの範囲の個人トークン> yomi deploy templates/blog --url https://yomi.example.com
```

- CLI はまずサーバーで試行し（何も書き込みません）、マニフェストの問題、破壊的な変更、壊れる登録済みのクエリを表示します。判定はサーバーが行い、デプロイのときにもう一度検査します。
- `--dry-run` は試行だけで終わります。
- テンプレートを変えてデプロイすると、それを使う**すべてのワークスペース**に反映されます。ワークスペース固有の構造が要るなら、別のテンプレートを作ってください。

### 破壊的な変更

フィールドの削除や型の変更は、`--allow-destructive` がなければ止まります。次の順に分けてください。

1. 新しいフィールドを追加してデプロイする。
2. データを移す。
3. 古いフィールドを削除してデプロイする。

### 登録済みのクエリを壊す変更

登録済みのクエリが壊れるワークスペースには、新しい版を適用しません。そのワークスペースは旧版のまま「移行待ち」になり、配信は続きますが、編集はできません。ほかのワークスペースには適用します。CLI は、壊れるクエリと、移行待ちになったワークスペースを表示し、終了コード 1 で終わります。

移行待ちになったワークスペースは、サイトのクエリを直して登録し直すか、使わないクエリを消した後に、もう一度デプロイすると（同じ内容でもかまいません）移ります。壊れるクエリがあっても適用するときは、`--allow-broken-queries` を付けます（そのクエリは失敗するようになります）。

同じテンプレートを使うすべてのワークスペースのクエリを、ワークスペースごとに確かめます。`register` のキーを持つ人が合わないクエリを登録しても、止まるのはそのワークスペースだけです。

長く使われていないクエリ（既定は90日）は検証しません。ビルドのたびの再登録も「使用」に数えます。

### 移行待ち

移行はワークスペースごとに行います。失敗したワークスペースは「移行待ち」になり、再試行されます。その間も配信は続きますが、編集と公開は止まります（`schema_migration_pending`）。

## 既存のブログからの取り込み

Markdown のフォルダから記事を取り込みます。

```bash
YOMI_TOKEN=<読むと書くの範囲の個人トークン> yomi import ./content --url https://yomi.example.com --workspace <ワークスペースID> --config import.json --redirects redirects.json --static-dir ./public
```

- すべてのファイルを先に検査し、1つでも問題があれば何も取り込みません。
- 取り込んだ記事は公開されます。元の URL は、新しい URL との対応として URL の変更履歴に残ります。
- `redirects.json` には、Nuxt の `routeRules`（元の URL のパス → 新しい URL のパス、301）を書き出します。
- 代替テキストのない画像は「飾り」として取り込み、警告を出します。
- 画像はフォルダの中のファイルだけを読みます（`/` で始まるパスは `--static-dir` の中）。外部の URL の画像は取り込めません。

設定ファイル（JSON、すべて省略可）:

| キー | 既定 | 内容 |
|---|---|---|
| `contentType` | コンテンツタイプが1つならそれ | 取り込み先のコンテンツタイプ |
| `originalUrlKey` | `url` | 元の URL を持つ frontmatter のキー（値は必須） |
| `localeKey` | `locale` | ロケールのキー（なければ既定のロケール） |
| `translationKey` | なし | 翻訳グループにまとめるキー |
| `translationOf` | なし | 元の記事のスラッグを指すキー |
| `bodyField` | 本文のフィールドが1つならそれ | 本文を入れるフィールド |
| `componentMap` | `{}` | 独自部品（`<Callout>`、`::callout`）→ 本文のノード。対応のない部品があれば止めます |
| `ignoreKeys` | `[]` | 取り込まない frontmatter のキー。フィールドにないキーは、ここに書かないと止めます |

**取り込む内容を確かめてください。** 元の URL がサイトと同じオリジンなら、そのページから記事への 301 ができます。CMS の外のページの URL を書くと、そのページが奪われます。取り込みは管理者と編集者だけが行えます。
