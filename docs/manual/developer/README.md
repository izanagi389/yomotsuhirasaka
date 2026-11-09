# 開発者向け手順書

テンプレート（記事の構造）とサイトを作る人のための手順書です。サーバーの導入は [サーバー管理者向け](../operator/README.md) にあります。

CMS はデータを返すだけで、公開サイトの見た目や部品は持ちません。サイトはサーバーの側で配信 API を呼びます。**ブラウザから配信 API や回答 API を呼ばせないでください。**

## 目次

1. [最初のサイトを作るまでの流れ](getting-started.md)
2. [テンプレートとCLI](templates-and-cli.md)（デプロイ、破壊的な変更、既存のブログからの取り込み）
3. [関数（URL生成関数・フック・変換関数）](functions.md)
4. [配信API](delivery-api.md)
5. [Nuxtモジュール](nuxt-module.md)
6. [Webhookの受け方と署名の書式](webhooks.md)
7. [回答API（チャットボット）](answer-api.md)

権限やエラーの意味は [共通の資料](../reference/README.md) を見てください。
