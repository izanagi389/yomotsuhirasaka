# @new-cms/nuxt

CMSの公式Nuxtモジュールです。配信APIからの取得、本文の描画、CMSのリダイレクト一覧による転送、CMSからのWebhookの受け口を持ちます（`llms.txt`・訂正フィード・プレビューなどはこれから）。

## 使い方

```ts
// nuxt.config.ts
export default defineNuxtConfig({
  modules: ["@new-cms/nuxt"],
  cms: {
    url: "https://cms.example.com",
    workspaceId: "<ワークスペースID>",
    // queriesDir: "cms/queries", routePrefix: "/_cms/q"
    siteUrl: "https://example.com", // リダイレクトを使うとき
    // webhookRoute: "/_cms/webhook"
  },
});
```

| 環境変数 | いつ | 内容 |
|---|---|---|
| `CMS_REGISTER_KEY` | ビルドのとき | クエリの登録用のAPIキー（`register` の範囲） |
| `NUXT_CMS_READ_KEY` | サイトのサーバーの実行時（SSG ならビルドのとき） | 読むためのAPIキー（`read` の範囲）。非公開の設定に入る |
| `NUXT_CMS_WEBHOOK_SECRET` | サイトのサーバーの実行時 | CMSのWebhookの署名の秘密。入れ替え中はカンマで区切って新旧の両方を書く。非公開の設定に入る |

### クエリ

`cms/queries/<名前>.graphql` に1ファイル1クエリで書きます。ビルドのたびに配信APIに登録し（本番の配信APIは登録済みのクエリだけを受け付けます）、名前で呼び出します。

```vue
<script setup lang="ts">
const { data } = await useCmsQuery<{ postBySlug: Post | null }>("postBySlug", { slug: route.params.slug, locale: "ja" });
</script>
```

- 取得はサイトのサーバーのルート（`/_cms/q/<名前>`）が行い、そこから配信APIを呼びます。**APIキーはブラウザに渡りません**（ブラウザから配信APIを直接呼ばせない、原則1）。
- 呼べるのは登録したクエリだけです。変数は JSON のオブジェクトで、大きさに上限があります。
- SSG では、ビルドのときの結果（payload）を使います。SSR では、リクエストのたびにサイトのサーバーが取ります。キャッシュはサイト側で持ちます（Q145）。
- キーやトークンらしい値が公開の設定（`runtimeConfig.public`）に入っていると、ビルドを止めます。

### 本文の描画

```vue
<CmsBody :doc="post.body" :components="{ embed: MyEmbed, image: MyImage, 'mark:link': MyLink }" />
```

- 本文（Tiptap の JSON）を、飾りのない意味のある HTML（`p`・`h2`・`ul`・`strong`・`a`・`img` など）として描きます。クラスもスタイルも付けません。見た目はサイトが決めます。
- `v-html` を使わず、文字はすべて文字として描きます。リンクと画像の URL は描く前にも確かめます。
- 見出しには、CMS が振った固定のアンカーIDを `id` に付けます。
- 埋め込みは、既定では元の URL へのリンクです。iframe にするかどうか（サイトの CSP など）はサイトが決め、`components` で置き換えます。
- 置き換える部品は `node`（マークなら `mark`）を受け取り、子は既定のスロットに入ります。

### リダイレクト（`siteUrl` を設定したとき）

CMSは、記事のURLが変わるたびに変更履歴を残し、配信APIでリダイレクト一覧を返します（連鎖は解消済みで、削除した記事のURLは 410）。このモジュールは、その一覧のうち `siteUrl` と同じオリジンのURLを転送します。

- ビルドのとき（`NUXT_CMS_READ_KEY` があれば）、301 を `routeRules` に足します。サイトが自分で書いた `routeRules` は上書きしません。
- サイトのサーバーのミドルウェアが、301 と 410 を返します。ビルドの後の変更も反映します。一覧はサーバーの中にキャッシュし（5分）、CMSのWebhookを受けたら消します。
- 静的ホスティング（サーバーなし）では 410 を返せません。ホスティングの転送の設定は、いまは書き出しません。
- リダイレクト一覧のクエリはモジュールが登録します。`cms/queries/cmsRedirects.graphql` という名前は使えません。

### CMSからのWebhookの受け口

CMSのWebhook（記事の公開・更新・非公開・削除・URLの変更など）の送信先に `https://<サイト>/_cms/webhook` を登録し、表示された秘密を `NUXT_CMS_WEBHOOK_SECRET` に設定します。

- 署名（HMAC-SHA256。送信時刻とイベントIDを含む）を確かめ、時刻が5分より離れたものと、同じイベントの再送は受け付けません。
- 確かめられたら、モジュールがサイトのサーバーに持つキャッシュ（リダイレクト一覧など）を消します。本文の中身は使いません。
- 秘密を設定しなければ、受け口は 404 を返します。
- サイトのサーバーが複数のプロセスで動くときは、Webhookを受けたプロセスのキャッシュだけが消えます。ほかのプロセスは5分以内に取り直します。

## 開発

```sh
npm run build -w @new-cms/nuxt   # dist/ を作る（サイトはこれを読む）
npm test -w @new-cms/nuxt
```

`playground/` は確かめ用のサイトです（`CMS_URL`・`CMS_WORKSPACE_ID`・`CMS_REGISTER_KEY`・`NUXT_CMS_READ_KEY` を設定して `npx nuxt generate`）。
