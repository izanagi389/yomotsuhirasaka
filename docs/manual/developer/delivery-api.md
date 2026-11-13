# 配信API

GraphQL の API です。サイトのサーバーから呼びます。**ブラウザから直接呼ばせないでください。** 公開のキーはありません。

Nuxt を使うなら、[Nuxtモジュール](nuxt-module.md) がこの章の内容を扱います。

## エンドポイント

| メソッド | パス | 内容 | キーの権限 |
|---|---|---|---|
| POST | `/ws/{ワークスペースID}/graphql/queries` | クエリを登録する（`{ "query": "…" }`）。応答は 201 で `{ "id": "…" }` | `register` |
| GET | `/ws/{ワークスペースID}/graphql?id=…&variables=…` | 登録済みのクエリを実行する | `read` |
| POST | `/ws/{ワークスペースID}/graphql` | 同じ（`{ "id": "…", "variables": { } }`）。回答と検索は POST だけ | `read`（回答は `answer`） |
| GET | `/ws/{ワークスペースID}/preview` | プレビュー用トークンを確かめる | `read` |

- キーは `Authorization: Bearer <キー>` でだけ受け付けます。URL に入っていれば断ります（`credential_in_url`）。送ってしまったキーは失効させてください。
- URL のワークスペースとキーのワークスペースが違えば断ります。
- 本番では**登録済みのクエリだけ**を実行します。イントロスペクションと複数クエリの一括送信はできません。
- 登録は、サイトのビルドやデプロイのときに行います（SSR のサイトも、閲覧のたびではなくデプロイのときに）。

## 名前の決まり方

コンテンツタイプ `blog_post` から、次の名前が決まります。

| 種類 | 名前 |
|---|---|
| 型 | `BlogPost` |
| 1件 | `blogPost(id: ID!, locale: String)` |
| スラッグで1件 | `blogPostBySlug(slug: String!, locale: String!)` |
| 一覧 | `blogPostList(locale: String!, limit: Int, offset: Int)` |

テンプレートのフィールドは名前をそのまま使います。CMS が決める名前とぶつかるテンプレートは、デプロイで止まります。

## 記事の型にCMSが置くフィールド

| フィールド | 内容 |
|---|---|
| `id`、`locale`、`url` | 記事の ID、ロケール、公開 URL |
| `publishedAt`、`firstPublishedAt` | 今の版と初回の公開日時 |
| `translationGroupId`、`alternates` | 翻訳グループと、ほかのロケールの記事 |
| `corrections`、`deletedAnchors` | 訂正の履歴と、削除されたセクション |
| `lastVerifiedAt`、`reviewDueAt`、`reviewExpired` | 鮮度 |
| `aiMarkdown`、`readingTime`、`aiTrainingOptOut` | AI 向けの素材 |
| `relatedEntries(first: Int)` | 関連記事（`searchable` なフィールドがあるときに計算されます） |

本文（`richtext`）は Tiptap の JSON で返ります。自分で描くときは、**すべての文字列をエスケープし、リンクの URL を確かめてください。**

画像のフィールド（`image`）は `{ url width height alt decorative }` で返ります（例: `thumbnail { url width height alt decorative }`）。メディアが削除されていれば `null` です。`decorative` が `true` の画像は、`alt=""` で描いてください。SNS の共有ボタンや OGP（`og:image` など）は、この値と題名・要約・URL からサイト側で作ります。

## ルートのフィールド

| フィールド | 内容 |
|---|---|
| `redirects` | リダイレクトの一覧（`from`、`to`、`status`）。連鎖は解消済みで、削除した記事は 410 |
| `corrections(locale, limit, offset)` | サイト全体の訂正フィード |
| `aiCatalog(locale, limit, offset)` | AI 向けの目録 |
| `aiMarkdownByUrl(url)` | 公開 URL から記事の Markdown 版 |
| `aiTrainingOptOuts` | 学習への利用を拒否した記事の URL |
| `search(text, locale, mode, first)` | サイト内検索。`mode` は `HYBRID`（既定）、`VECTOR`、`KEYWORD`、`first` は50まで、`text` は500文字まで。検索語を URL に載せない（プロキシやアクセスログに残さない）ため、POST でだけ実行できる（GET は `search_requires_post`） |
| `answer`、`answerStream` | [回答API](answer-api.md) |

検索の応答の `vectorSearch` が `BUDGET_EXHAUSTED` なら訪問者の分の埋め込みの予算切れ（キーワード検索の結果だけ）、`keywordSearch` が `DISABLED` なら日本語の全文検索の拡張がないサーバーです。

非公開、削除、対象外の記事は、返すたびに今の状態と照合して落とします。

## 上限

| 項目 | 既定 |
|---|---|
| クエリの深さ | 15（登録のときに断ります） |
| クエリの複雑さ（選ぶフィールドの数） | 500（登録のときに断ります） |
| キーごとの回数（用途がビルド用） | 1分600回 |
| キーごとの回数（用途がサーバー用） | 1分6000回 |
| ワークスペースごとの回数 | 1分30000回 |
| 応答の大きさ | 5MB |

値はサーバーの設定 [`[delivery]`](../operator/config.md#配信apidelivery) で決まります。回数の制限を超えると 429 と `Retry-After` を返します。

## キャッシュ

応答は既定で `Cache-Control: private` です。サイトの側でキャッシュしてください（[キャッシュについて](getting-started.md#キャッシュについて)）。

サーバー管理者が共有キャッシュを有効にすると（[共有キャッシュ](../operator/config.md#共有キャッシュ)）、`read` の範囲だけのキーの応答に `s-maxage` が付きます。CDN に載った応答は、**キーの照合なしに URL を知っている人なら誰でも取れます**。また、記事を非公開にしても、CDN に残った応答は CMS から消せません。必ず Webhook で CDN のキャッシュを消してください。

## プレビュー

読み取り用のキーに加えて `X-Yomi-Preview-Token` ヘッダーでプレビュー用トークンを送ると、その記事だけが下書きで返ります（1件を取るクエリだけ）。トークンだけでは使えません。応答は `Cache-Control: private, no-store` です。**共有キャッシュやサイトのキャッシュに載せないでください。**

## エラー

入口のエラーは [エラーコード](../reference/error-codes.md) を見てください。内部の詳細（SQL、スタックトレース、内部の名前）は返しません。
