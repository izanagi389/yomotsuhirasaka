# @new-cms/nuxt

CMSの公式Nuxtモジュールです。M1では、配信APIからの取得と、本文の描画だけを行います（`llms.txt`・訂正フィード・プレビューなどは M2）。

## 使い方

```ts
// nuxt.config.ts
export default defineNuxtConfig({
  modules: ["@new-cms/nuxt"],
  cms: {
    url: "https://cms.example.com",
    workspaceId: "<ワークスペースID>",
    // queriesDir: "cms/queries", routePrefix: "/_cms/q"
  },
});
```

| 環境変数 | いつ | 内容 |
|---|---|---|
| `CMS_REGISTER_KEY` | ビルドのとき | クエリの登録用のAPIキー（`register` の範囲） |
| `NUXT_CMS_READ_KEY` | サイトのサーバーの実行時（SSG ならビルドのとき） | 読むためのAPIキー（`read` の範囲）。非公開の設定に入る |

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

## 開発

```sh
npm run build -w @new-cms/nuxt   # dist/ を作る（サイトはこれを読む）
npm test -w @new-cms/nuxt
```

`playground/` は確かめ用のサイトです（`CMS_URL`・`CMS_WORKSPACE_ID`・`CMS_REGISTER_KEY`・`NUXT_CMS_READ_KEY` を設定して `npx nuxt generate`）。
