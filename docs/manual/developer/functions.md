# 関数（URL生成関数・フック・変換関数）

テンプレートには、TypeScript で関数を書けます。CLI が JavaScript にまとめ、サーバーの組み込みの JS ランタイム（QuickJS 系）で動かします。

動くのは、テンプレートに書かれ、サーバー管理者がデプロイしたコードだけです。**デプロイの権限は、サーバーの上でコードを動かす権限と同じです。** 公開サイトのサーバーでは動きません。サイトへの通知は Webhook を使ってください。

## 書き方

関数はテンプレートのフォルダの中のファイルに書き、`template.json` からパスで参照します。既定のエクスポートが関数の本体です。

```json
{
  "contentTypes": [
    {
      "name": "post",
      "urlFunction": { "file": "./functions/url.ts", "permissions": ["content.read"] },
      "hooks": {
        "afterPublish": { "file": "./functions/notify.ts", "permissions": ["http.fetch"], "allowedHosts": ["hooks.example.com"] }
      }
    }
  ]
}
```

```ts
// functions/url.ts
import type { UrlFunction } from "@yomotsuhirasaka/cli/types";

const url: UrlFunction = (entry) => `https://example.com/${entry.locale}/blog/${entry.slug}`;
export default url;
```

- `import` できるのは、テンプレートのフォルダの中の相対パスだけです。npm のパッケージや `node:` は使えません。
- 型は `@yomotsuhirasaka/cli/types` にあります（`cli/types/index.d.ts`）。型だけなので、変換のときに消えます。
- フォルダの外を指すパス（`..`、絶対パス、外に出るシンボリックリンク）は止めます。

## 関数の種類

| 種類 | 宣言する場所 | 受け取るもの | 返すもの | 失敗したとき |
|---|---|---|---|---|
| URL生成関数 | `urlFunction` | 記事（`slug` 付き） | http(s) の絶対 URL | 公開を止める（`url_function_failed`） |
| 保存前のフック | `hooks.beforeSave` | `{ entry }` | `{ fields }`（直すフィールド） | 直さずに保存し、編集者に知らせる |
| 公開前のフック | `hooks.beforePublish` | `{ entry }` | `{ errors: [{ field, message }] }` | 公開を止める（`hook_failed`、`publish_rejected_by_hook`） |
| 保存後のフック | `hooks.afterSave` | `{ event, entry }` | 使わない | やり直す |
| 公開後のフック | `hooks.afterPublish` | `{ event, entry }` | 使わない | やり直す |
| 非公開の後のフック | `hooks.afterUnpublish` | `{ event, entry }` | 使わない | やり直す |
| 変換関数 | `body.customNodes[].toMarkdown`、`toHtml` | `(node, ctx)` | 文字列 | デプロイの検査か公開で止める（`conversion_failed`） |

- 保存前のフックは保存を止められません。下書きを失わせないためです。
- `async` の関数は使えません。戻り値を使う関数（URL生成関数、保存前と公開前のフック、変換関数）が Promise や JSON にできない値を返すと失敗として扱います。公開前のフックなら公開を止めます（`hook_failed`）。
- 非同期のフックはジョブで動き、失敗すると5回まで試します。**同じイベントが2回届くことがあります。** `event.id` で見分けてください。
- 保存後のフックは、記事ごとにまとめて、保存が止まってから1回だけ動きます。
- フックのジョブは、ワークスペースごとに1つずつ実行します。

## ホスト関数と権限

関数から CMS の機能を呼べるのは、ホスト関数だけです。使うものを `permissions` に宣言します。宣言していないものは呼べません。

| 権限 | 関数 | 内容 |
|---|---|---|
| `content.read` | `yomi.content.get(id)` | 同じワークスペースの公開中の記事（`{ id, contentType, locale, fields }`）。なければ `null` |
| `secrets.read` | `yomi.secrets.get(name)` | `secrets` に宣言した名前の秘密の値（文字列）。登録がなければ `null`。非同期のフックだけで使えます |
| `http.fetch` | `yomi.http.fetch(url, { method, headers, body })` | `{ status, body }` を返す。本文は 1MB まで。リダイレクトはたどらない。失敗は例外 |

- `http.fetch` は非同期のフックだけで使えます。同期の処理（URL生成関数、保存前、公開前）で外部と通信することはできません。
- 宛先は `allowedHosts` に書いたものだけです。外部への送信の共通の仕組みを通るので、内部向けのアドレスには送れません。
- 他のワークスペースのデータには、どの関数でも触れられません。
- **API キーなどの秘密を、関数のソースに書かないでください。** ソースはリポジトリと CMS のデータベースに平文で残ります。ワークスペースの管理者が管理画面の「フックの秘密」に登録し、フックの `permissions` に `secrets.read`、`secrets` に使う名前を書きます。
- URL生成関数が `yomi.content.get` で読んだ記事は自動で記録され、読んだ記事が変わると URL を計算し直します。

## 制限

1回の実行ごとに、次の上限があります。超えると打ち切ります。値はサーバーの設定 [`[scripts]`](../operator/config.md#組み込みjsscripts) で決まります。

| 項目 | 既定 |
|---|---|
| メモリ | 128MB |
| 同期の実行の時間（URL生成関数、保存前、公開前） | 5秒 |
| 非同期のフックの時間 | 30秒 |

将来の実行環境の変更に備えて、JSON を受け取って JSON を返す関数にしてください（変換関数は、JSON を受け取って文字列を返す純粋な関数）。QuickJS に固有の機能やグローバル変数に頼らないでください。

## カスタムノード

本文に独自の部品を足せます。雛形は次のコマンドで作ります。`template.json` は書き換えず、足す宣言を表示します。

```bash
yomi node scaffold callout --template templates/blog
```

インラインの部品なら `--inline` を付けます。

```json
{ "body": { "customNodes": [
  { "name": "callout", "attrs": [{ "name": "label", "type": "string" }], "toMarkdown": { "file": "./nodes/callout.ts" } }
] } }
```

- `toMarkdown` は必須です。AI 向けの素材、検索、回答に使います。
- `toHtml` は任意です。返した HTML は無害化されます。
- `ctx.children()` は、子のノードを同じ形式に変換したものを返します。
