# @new-cms/nuxt

CMSの公式Nuxtモジュールです。配信APIからの取得、本文の描画、CMSのリダイレクト一覧による転送、CMSからのWebhookの受け口、訂正の一覧とフィード、削除済みアンカーの案内、AI向けの素材（記事のMarkdown版・`llms.txt`・学習拒否）、サイト内検索、チャットボットの回答の中継を持ちます（プレビューなどはこれから）。

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
    // AI向けの素材を出すとき（siteUrl が要る）
    ai: { locales: { ja: { title: "例のブログ", description: "技術の記事" } }, robotsTxt: {} },
  },
});
```

| 環境変数 | いつ | 内容 |
|---|---|---|
| `CMS_REGISTER_KEY` | ビルドのとき | クエリの登録用のAPIキー（`register` の範囲） |
| `NUXT_CMS_READ_KEY` | サイトのサーバーの実行時（SSG ならビルドのとき） | 読むためのAPIキー（`read` の範囲）。非公開の設定に入る |
| `NUXT_CMS_WEBHOOK_SECRET` | サイトのサーバーの実行時 | CMSのWebhookの署名の秘密。入れ替え中はカンマで区切って新旧の両方を書く。非公開の設定に入る |
| `NUXT_CMS_ANSWER_KEY` | サイトのサーバーの実行時 | 回答のAPIキー（`answer` の範囲。`answer` を設定したとき）。お金がかかるので、サイトのサーバーだけに置く。非公開の設定に入る |

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

### AI向けの素材（`ai` を設定したとき）

CMSが作る素材（記事のMarkdown版、目録、学習への利用を拒否した記事）を、サイトのドメインに置きます。どの記事が素材になるかは、CMSのスキーマ（コンテンツタイプの `aiMarkdown`）と、記事の「外部AI向けの素材に含める」で決まります。

- `<記事のURL>.md`: 記事のMarkdown版（例: `/ja/blog/hello` → `/ja/blog/hello.md`。末尾の `/` は除きます）。`siteUrl` と同じオリジンの記事だけです。目録にないパスは、サイトの処理に任せます。
- `/llms.txt`: `locales` のロケールごとに、記事のMarkdown版へのリンクを並べます。先頭の名前と説明は最初のロケールのものです。
- `/robots.txt`（`robotsTxt` を設定したとき）: 学習用のクローラーに、学習への利用を拒否した記事とそのMarkdown版を `Disallow` で並べます。モジュールが robots.txt 全体を返すので、`public/robots.txt` があるとビルドを止めます。サイトの行は `extra` に書きます。

| 設定 | 内容 |
|---|---|
| `locales` | ロケール → `{ title, description }`（llms.txt に書く） |
| `markdown` | `.md` を返すか（既定は true） |
| `llmsTxt` | `/llms.txt` を返すか（既定は true） |
| `robotsTxt` | `{ addCrawlers, removeCrawlers, extra }`。既定のクローラーは `DEFAULT_AI_TRAINING_CRAWLERS`（GPTBot、Google-Extended、CCBot、ClaudeBot、anthropic-ai、Applebot-Extended、meta-externalagent、Bytespider）。検索・引用のためのクローラーは含めません |

- サーバーのルートが返し、配信APIから取ったものはキャッシュします（5分。CMSのWebhookを受けたら消します）。
- `nuxt generate` では、ビルドのときに `llms.txt`・`robots.txt`・記事の `.md` をファイルとして書き出します（記事の `.md` には、ビルドのときの `NUXT_CMS_READ_KEY` が要ります）。
- `robots.txt` で学習用のクローラーの組を作ると、そのクローラーは `User-agent: *` の組を読まなくなります。`extra` の `*` の規則を学習用のクローラーにも効かせたいときは、`extra` にそのクローラーの組も書いてください。

学習への利用を拒否した記事のページには、メタタグを足します。記事のクエリに `aiTrainingOptOut` を足し、記事のページで呼びます。

```vue
<script setup lang="ts">
const { data } = await useCmsQuery<{ postBySlug: Post | null }>("postBySlug", { slug, locale: "ja" });
useCmsAiMeta(() => data.value?.postBySlug); // <meta name="robots" content="noai, noimageai">
</script>
```

これらはクローラーへの意思表示であって、技術的に止めるものではありません。

### サイト内検索（`search` を設定したとき）

CMSの検索（ベクトル検索とキーワード検索の組み合わせ）を、サイトのサーバーのルート（既定は `/_cms/search`）を通して使います。ブラウザから配信APIは呼びません。サイトのサーバーが要るので、`nuxt generate` では使えません（ビルドを止めます）。

```ts
// nuxt.config.ts
cms: { search: { rateLimit: { perMinute: 30, ipHeader: "cf-connecting-ip" } } }
```

```vue
<!-- 部品を使う（見た目はサイトが決める。文言はすべてサイトが渡す） -->
<CmsSearchBox label="サイト内を検索" locale="ja">
  <template #loading>検索しています…</template>
  <template #empty>見つかりませんでした。</template>
  <template #rateLimited>しばらくしてからもう一度お試しください。</template>
</CmsSearchBox>
```

```vue
<!-- 自分で描く -->
<script setup lang="ts">
const { text, result, status } = useCmsSearch({ locale: "ja", mode: "HYBRID", first: 10 });
</script>

<template>
  <input v-model="text" type="search" aria-label="サイト内を検索" />
  <ol v-if="result">
    <li v-for="hit in result.hits" :key="hit.entryId">
      <a :href="`${hit.url}#${hit.anchor}`">{{ hit.breadcrumb.join(" › ") }}</a>
      <p><template v-for="(s, i) in cmsHighlight(hit.excerpt, hit.matches)" :key="i"><mark v-if="s.match">{{ s.text }}</mark><template v-else>{{ s.text }}</template></template></p>
    </li>
  </ol>
</template>
```

| 設定 | 内容 |
|---|---|
| `route` | サイトのサーバーのルート（既定は `/_cms/search`）。`?q=&locale=&mode=&first=` で呼ぶ |
| `rateLimit.perMinute` | 訪問者のIPアドレスごとの1分あたりの回数（既定は 30）。超えると 429 |
| `rateLimit.ipHeader` | 訪問者のIPアドレスを読むヘッダー（例: `cf-connecting-ip`、`x-forwarded-for`）。既定は空で、接続元のアドレスを使う |

- 回数の制限は、訪問者の入力のたびにCMSが呼ばれ、ベクトル検索ならワークスペースの埋め込みの予算も減るためです。数えるのはサイトのサーバーのメモリの中だけです（プロセスごと）。
- CDN やリバースプロキシの後ろでは、接続元のアドレスが全員同じになり、訪問者全員で1つの上限を共有します。手前のプロキシが付けるヘッダーを `ipHeader` に書いてください（`x-forwarded-for` はいちばん右の値を使います）。訪問者が偽れるヘッダーは書かないでください。本番のビルドで空のままなら、ビルドのログに注意を出します。
- 検索語は 500 文字まで。空の検索語では CMS を呼ばずに空の結果を返します。同じ検索の結果は 60 秒だけサーバーの中に覚えておき、CMSのWebhookを受けたら消します。応答は `Cache-Control: private, no-store` です。
- `useCmsSearch` の `locale` は必須です（サイトのロケールの決め方はサイトごとに違うので、モジュールは推測しません）。既定は `HYBRID` で 10 件（上限 50）。入力が止まってから（既定 300 ミリ秒）検索し、後から始めた検索の結果を古い結果で上書きしません。`status` は `idle`・`loading`・`done`・`error`・`rateLimited` です。
- `<CmsSearchBox>` は、`<search>` の中にラベル付きの検索欄と結果の `<ol>` を描きます。クラスもスタイルも、決まった文言も持ちません。状態はスロット（`loading`・`empty`・`budgetExhausted`・`keywordDisabled`・`rateLimited`・`error`）でだけ描きます。結果1件は `#hit="{ hit, href, segments }"` で置き換えられます。既定は、パンくず（なければURL）のリンクと、一致した箇所を `<mark>` で囲んだ抜粋です。
- 結果の `breadcrumb` に記事のタイトルが入るのは、タイトルのフィールドが検索の対象のときだけです。`vectorSearch` が `BUDGET_EXHAUSTED` なら埋め込みの予算切れ（キーワード検索の結果だけ）、`keywordSearch` が `DISABLED` なら日本語の全文検索の拡張がないCMSです。
- 検索のクエリはモジュールが登録します（`cms/queries/cmsSearch.graphql` という名前は使えません）。

### チャットボットの回答（`answer` を設定したとき）

CMSの回答API（判定先行のストリーミング）を、サイトのサーバーのルート（既定は `POST /_cms/answer`）が中継します。ブラウザからCMSは呼びません。回答のキーは `NUXT_CMS_ANSWER_KEY`（`answer` の範囲。非公開の設定に入ります）です。サイトのサーバーが要るので、`nuxt generate` では使えません（ビルドを止めます）。チャットの画面の部品は配布しません。画面はサイトが作ります。

```ts
// nuxt.config.ts
cms: { answer: { ipHeader: "cf-connecting-ip" } }
```

```vue
<script setup lang="ts">
const { state, ask, stop } = useCmsAnswer();
const question = ref("");
</script>

<template>
  <form @submit.prevent="ask(question, 'ja')">
    <input v-model="question" maxlength="500" aria-label="質問" />
  </form>
  <CmsAnswerText v-if="state.text" :text="state.text" :sources="state.sources" />
  <ol v-if="state.phase === 'done'">
    <li v-for="s in state.sources" :key="s.citation"><a :href="`${s.url}#${s.anchor}`">[{{ s.citation }}] {{ s.breadcrumb.join(" › ") }}</a></li>
  </ol>
  <p v-if="state.error?.code === 'ANSWER_BUDGET_EXHAUSTED'">いまは回答できません。</p>
</template>
```

| 設定 | 内容 |
|---|---|
| `route` | サイトのサーバーのルート（既定は `/_cms/answer`）。`POST` で JSON の `{ "question": "…", "locale": "ja" }` を送る |
| `ipHeader` | 訪問者のIPアドレスを読むヘッダー（検索と同じ。手前のプロキシが付けるものだけ） |

- ルートは SSE で、`verdict`（判定。`answerable` が false なら本文は来ない）→ `delta`（本文の断片）→ `final`（最終判定）の順にイベントを返します。途中で `error` が来たら終わりです。
- **回答キャッシュから返したか（`cached`）は渡しません**（クエリでも選びません）。CMSのエラーのメッセージも渡さず、コード（`RATE_LIMITED`・`ANSWER_BUDGET_EXHAUSTED`・`QUESTION_EMPTY`・`QUESTION_TOO_LONG`・`QUESTION_REFUSED`・`LOCALE_NOT_SUPPORTED`・`LLM_UNAVAILABLE`・`ANSWER_FAILED`）だけを渡します。ストリームが始まる前に断られたら、状態コードと `{ "code": "…" }` を返します（回数の制限は 429 と `Retry-After`）。
- 訪問者ごとの回数は CMS が数えます。訪問者の識別子は、訪問者のIPアドレスを回答のキーで HMAC-SHA256 にかけたもので、IPアドレスそのものは CMS に送りません。CDN やリバースプロキシの後ろでは `ipHeader` を設定してください（しなければ、訪問者全員で1つの上限を共有します）。
- 回答は Markdown です。`<CmsAnswerText>` は段落・箇条書き・太字・リンク（http・https だけ）・インラインのコード・コードのかたまりだけを、`v-html` を使わずに描きます。画像は描きません。見出しは太字の段落になります。クラスもスタイルも付けません。
- 本文の `[n]` は、`citation` が同じ出典のセクションへのリンク（`<sup><a href="url#anchor">[n]</a></sup>`）です。`#citation="{ n, source, href }"` のスロットで置き換えられます。
- 質問のロケールは必ず渡します（CMSは質問文からロケールを推測しません）。応答は `Cache-Control: no-store` です。
- 回答のクエリはモジュールが登録します（`cms/queries/cmsAnswer.graphql` という名前は使えません）。

### 関連記事

関連記事は、記事のクエリに `relatedEntries` を足せば取れます（モジュールの設定は要りません）。非公開・削除された記事は、CMSが返すときに落とします。

```graphql
postBySlug(slug: $slug, locale: $locale) { title body relatedEntries(first: 5) { id url title pinned } }
```

- 同じコンテンツタイプ・同じロケールの記事だけが返ります。編集者が固定した記事が先頭です。
- CMSは毎日の一括計算の後などに、一覧が変わった記事を `entries.related_changed` のWebhookで知らせます。このモジュールは関連記事を覚えていないので、受け口ではほかのイベントと同じくキャッシュを消すだけです。ページをキャッシュしているサイト（ISR など）は、この知らせで該当する記事のページを作り直してください。

## 開発

```sh
npm run build -w @new-cms/nuxt   # dist/ を作る（サイトはこれを読む）
npm test -w @new-cms/nuxt
```

`playground/` は確かめ用のサイトです（`CMS_URL`・`CMS_WORKSPACE_ID`・`CMS_REGISTER_KEY`・`NUXT_CMS_READ_KEY` を設定して `npx nuxt generate`）。
