# @new-cms/nuxt

CMSの公式Nuxtモジュールです。配信APIからの取得、本文の描画、CMSのリダイレクト一覧による転送、CMSからのWebhookの受け口、訂正の一覧とフィード、削除済みアンカーの案内を持ちます（`llms.txt`・プレビューなどはこれから）。

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
    // 訂正の一覧とフィードを使うとき（キーはロケール）
    corrections: { feeds: { ja: { path: "/corrections", title: "訂正の一覧", link: "https://example.com/" } } },
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

### 訂正の一覧とフィード（`corrections` を設定したとき）

「訂正として公開」した記事の訂正を、新しい順に出します（通常の更新や確認済みの記録は含みません）。変更前の本文や差分は、配信APIも返しません。

```vue
<!-- pages/corrections.vue（ページはサイトが作る） -->
<script setup lang="ts">
const { data } = await useCmsCorrections({ locale: "ja", limit: 20, offset: 0 });
</script>

<template>
  <CmsCorrections :items="data?.corrections" />
</template>
```

- `<CmsCorrections>` は、クラスもスタイルもない `ol`／`li` に、日付（`time`。UTC の YYYY-MM-DD）、記事へのリンク、説明文（`p`）を描きます。1件ごとの描き方は `#item="{ item }"` のスロットで置き換えられます（`item.sections` は影響したセクションのアンカーID）。
- 説明文が未記入の訂正には、ワークスペースの定型文（未記入のときの文）が入ります。そのロケールの定型文がなければ `null` です。
- `corrections: {}`（フィードなし）でも、一覧の composable と部品は使えます。訂正の一覧のクエリはモジュールが登録します（`cms/queries/cmsCorrections.graphql` という名前は使えません）。

フィードは、`feeds` のロケールごとに `<path>.xml`（RSS 2.0）と `<path>.json`（JSON Feed 1.1）を返します。

| 設定 | 内容 |
|---|---|
| `path` | パス（拡張子なし。`/corrections` なら `/corrections.xml` と `/corrections.json`） |
| `title` | フィードの名前 |
| `link` | サイトのURL |
| `description` | フィードの説明（省略すると `title`） |
| `limit` | 載せる件数（1〜100。既定は 50） |

- サイトのサーバーのルートが返し、配信APIから取ったものはキャッシュします（5分。CMSのWebhookを受けたら消します）。
- `nuxt generate` では、ビルドのときにファイルとして書き出します。
- 項目のIDは「記事のID@訂正の日時」です。JSON Feed では、影響したセクションを拡張の `_corrections.sections` に入れます。

### 削除済みアンカーの案内

古いリンクの `#アンカー` が、訂正などで消えたセクションのものなら知らせます。記事のクエリに `deletedAnchors` を足し、記事のページに部品を置きます。

```graphql
postBySlug(slug: $slug, locale: $locale) { title body deletedAnchors { anchor deletedAt description } }
```

```vue
<CmsDeletedAnchorNotice :anchors="post.deletedAnchors" />
<!-- 説明文がないとき（通常の更新で消えたときなど）の文を渡す -->
<CmsDeletedAnchorNotice :anchors="post.deletedAnchors" v-slot="{ anchor }">{{ anchor.description ?? "このセクションは削除されました" }}</CmsDeletedAnchorNotice>
```

- `#` 以降はサーバーに届かないので、ブラウザで照らし合わせます（表示したときと、`#` が変わったとき）。一覧はサイトのサーバーが記事と一緒に取ったものを使い、ブラウザから配信APIは呼びません。
- サーバーでの描画では何も出しません。出すときは `<aside role="status">` で、クラスもスタイルも付けません。
- 説明文もスロットもなければ、何も出しません。

## 開発

```sh
npm run build -w @new-cms/nuxt   # dist/ を作る（サイトはこれを読む）
npm test -w @new-cms/nuxt
```

`playground/` は確かめ用のサイトです（`CMS_URL`・`CMS_WORKSPACE_ID`・`CMS_REGISTER_KEY`・`NUXT_CMS_READ_KEY` を設定して `npx nuxt generate`）。
