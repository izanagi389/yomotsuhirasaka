# 最初のサイトを作るまでの流れ

サーバーはすでに動いているものとします（[導入](../operator/install.md)）。

## 1. CLI とモジュールをソースからビルドする

CLI と Nuxt モジュールは npm では公開していません。リポジトリのソースからビルドします。リリースのタグを取り出して使ってください。Node.js は 22.12 以上が要ります。

```bash
git clone https://github.com/izanagi389/yomotsuhirasaka.git
```

```bash
cd yomotsuhirasaka && git checkout v1.0.0
```

```bash
npm ci
```

```bash
npm run build -w @yomotsuhirasaka/preview -w @yomotsuhirasaka/nuxt -w @yomotsuhirasaka/editor-schema -w @yomotsuhirasaka/cli
```

CLI は `node cli/dist/index.js` で動きます。以下ではこれを `yomi` と書きます。

Nuxt モジュールは、次のコマンドで作るファイルをサイトに入れます。入れ方は [Nuxtモジュール](nuxt-module.md#入れ方) にあります。

```bash
mkdir -p ../yomi-packages && npm pack -w @yomotsuhirasaka/preview -w @yomotsuhirasaka/nuxt --pack-destination ../yomi-packages
```

## 2. テンプレートを作ってデプロイする

`templates/<テンプレート名>/template.json` に記事の構造を書きます。例は `templates/blog/template.json` です。デプロイはサーバー管理者が、デプロイの範囲の個人トークンで行います。

見本の `blog` をそのまま使うなら、サーバー管理者が管理画面の「サーバーの管理」→「テンプレート」からデプロイできます（CLI は要りません。[ワークスペースと利用者](../operator/workspaces.md#同梱の見本blogを管理画面からデプロイする)）。見本を変えたテンプレートは、CLI でデプロイします。

```bash
YOMI_TOKEN=<個人トークン> yomi deploy templates/blog --url https://yomi.example.com
```

詳しくは [テンプレートとCLI](templates-and-cli.md) を見てください。

## 3. ワークスペースを作る

サーバー管理者が、管理画面の「新しいワークスペース」で、テンプレートの名前を指定して作ります（[ワークスペースの作成](../operator/workspaces.md)）。ワークスペースの ID は URL（`/admin/w/<ワークスペースID>/`）で分かります。

## 4. 配信APIのキーを発行する

ワークスペースの管理者が、「管理」→「配信APIのキー」で発行します。キーは発行のときに一度だけ表示されます。

| キー | 権限 | 用途 | 置く場所 |
|---|---|---|---|
| 登録用 | クエリの登録 | ビルド用 | CI のビルドの環境 |
| 読み取り用 | 読む | ビルド用（SSG）かサーバー用（SSR） | サイトのサーバー |

権限を分けておくと、片方が漏れたときの影響を小さくできます。

## 5. サイトを作る

[Nuxtモジュール](nuxt-module.md) を使うのが近道です。ほかのフレームワークでは、[配信API](delivery-api.md) をサイトのサーバーから直接呼びます。

## 6. Webhook をつなぐ

記事の公開や訂正をサイトのキャッシュに反映するため、「管理」→「Webhook」にサイトの受け口を登録します（[Webhookの受け方](webhooks.md)）。

## キャッシュについて

サイトは SSG、ISR、SSR のどれでもかまいません。**SSR では、必ずサイトの側でキャッシュしてください。** 閲覧のたびに CMS へ問い合わせると、回数の制限に当たりやすくなります。Nuxt なら `routeRules` の `swr` を使います。

```ts
// nuxt.config.ts
routeRules: { "/blog/**": { swr: 300 } }
```

記事の変更は Webhook で知らされるので、受けたときに該当のキャッシュを消してください。
