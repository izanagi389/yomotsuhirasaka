# 回答API（チャットボット）

公開済みの記事だけに基づいて、出典付きで訪問者の質問に答える API です。画面の部品は配布しません。画面はサイトが作ります。

**サーバー間でだけ呼んでください。** ブラウザから呼べる公開のキーはありません。回答のキーを持つのはサイトのサーバーだけです。呼ぶたびに LLM の料金がかかります（ワークスペースの「訪問者向けの回答」の予算の範囲で）。

## 使う前に

- サーバー管理者が、`[llm] chat_model` と回答の予算の天井を設定します（[LLMの設定](../operator/llm.md)）。
- 記事を根拠に使うには、テンプレートのフィールドに `searchable: true` が要ります。
- 「回答API」の権限のキーを、管理の「配信APIのキー」で発行します。

## Nuxt モジュールで使う

`answer` を設定すると、サイトのサーバーのルート（既定は `POST /_yomi/answer`）が CMS を呼び、SSE のまま中継します。回答のキーは `NUXT_YOMI_ANSWER_KEY` に入れます（非公開の設定に入ります）。サイトのサーバーが要るので、`nuxt generate` では使えません（ビルドを止めます）。

```ts
// nuxt.config.ts
yomi: { answer: { ipHeader: "cf-connecting-ip" } }
```

```vue
<script setup lang="ts">
const { state, ask, stop } = useYomiAnswer();
const question = ref("");
</script>

<template>
  <form @submit.prevent="ask(question, 'ja')">
    <input v-model="question" maxlength="500" aria-label="質問" />
  </form>
  <YomiAnswerText v-if="state.text" :text="state.text" :sources="state.sources" />
  <ol v-if="state.phase === 'done'">
    <li v-for="s in state.sources" :key="s.citation"><a :href="`${s.url}#${s.anchor}`">[{{ s.citation }}] {{ s.breadcrumb.join(" › ") }}</a></li>
  </ol>
  <p v-if="state.error?.code === 'ANSWER_BUDGET_EXHAUSTED'">いまは回答できません。</p>
</template>
```

| 設定 | 内容 |
|---|---|
| `route` | サイトのサーバーのルート（既定は `/_yomi/answer`）。`POST` で JSON の `{ "question": "…", "locale": "ja" }` を送る |
| `ipHeader` | 訪問者の IP アドレスを読むヘッダー。手前の CDN やリバースプロキシが付けるものだけを書く |

- ルートは SSE で、`verdict`（判定。`answerable` が false なら本文は来ない）→ `delta`（本文の断片）→ `final`（最終判定）の順にイベントを返します。途中で `error` が来たら終わりです。
- ブラウザに渡すエラーはコードだけです: `RATE_LIMITED`、`ANSWER_BUDGET_EXHAUSTED`、`QUESTION_EMPTY`、`QUESTION_TOO_LONG`、`QUESTION_REFUSED`、`LOCALE_NOT_SUPPORTED`、`LLM_UNAVAILABLE`、`ANSWER_FAILED`。ストリームが始まる前に断られたら、状態コードと `{ "code": "…" }` を返します（回数の制限は 429 と `Retry-After`）。
- 訪問者の識別子は、訪問者の IP アドレスを回答のキーで HMAC-SHA256 にかけたものです。IP アドレスそのものは CMS に送りません。CDN やリバースプロキシの後ろでは `ipHeader` を設定してください。しないと、訪問者全員で1つの上限を共有します。
- `<YomiAnswerText>` は、段落、箇条書き、太字、リンク（http、https だけ）、インラインのコード、コードのかたまりだけを、`v-html` を使わずに描きます。画像は描きません。見出しは太字の段落になります。クラスもスタイルも付けません。
- 本文の `[n]` は、`citation` が同じ出典のセクションへのリンク（`<sup><a href="url#anchor">[n]</a></sup>`）です。`#citation="{ n, source, href }"` のスロットで置き換えられます。
- 応答は `Cache-Control: no-store` です。回答のクエリはモジュールが登録します（`yomi/queries/yomiAnswer.graphql` という名前は使えません）。

## 自分で中継する

### 呼び方

配信 API の登録済みのクエリとして、**POST で**呼びます（GET は `answer_requires_post`）。`answer` の範囲のキーが要ります。`read` のキーでは呼べません。

```graphql
query Ask($q: String!, $locale: String!, $visitor: String) {
  answer(question: $q, locale: $locale, visitorId: $visitor) {
    status text truncated
    sources { citation entryId locale url anchor breadcrumb versionId lastReviewedAt reviewExpired }
  }
}

subscription AskStream($q: String!, $locale: String!, $visitor: String) {
  answerStream(question: $q, locale: $locale, visitorId: $visitor) {
    __typename
    ... on YomiAnswerVerdict { answerable sources { citation entryId locale url anchor breadcrumb versionId lastReviewedAt reviewExpired } }
    ... on YomiAnswerDelta { text }
    ... on YomiAnswerFinal { status text truncated sources { citation entryId locale url anchor breadcrumb versionId lastReviewedAt reviewExpired } }
  }
}
```

- `answerStream` は SSE で返します（GraphQL over SSE の「別々の接続」の形。`event: next` の data に GraphQL の応答、最後に `event: complete`）。
- 質問は500文字までです。**ロケールは必ず渡してください。** CMS は質問文からロケールを推測しません。
- 回答は Markdown です。**表示する前に必ずエスケープしてください。** `v-html` や `innerHTML` に直接入れないでください。

### 訪問者の識別子

`visitorId` には、訪問者ごとの値を**ハッシュにして**渡します（128文字まで）。CMS は回数を数えるだけで、保存しません。渡さないと、1人の訪問者が予算を使い切っても止められません。

### 中継で必ず守ること

- **`cached`（回答キャッシュから返したか）を訪問者に渡さないでください。** 誰かが同じ質問をしたかどうかが分かってしまいます。Nuxt モジュールはクエリでも選びません。
- CMS のエラーのメッセージをそのまま渡さず、コードから自分の文言を出してください。
- サイトの側でも回数を制限してください。
- 回答の応答をキャッシュしないでください。

## 回答の中身

- 記事に答えがなければ `NO_INFORMATION` で、本文はワークスペースの定型文です。LLM の一般知識では答えません。
- 引用のない回答は、CMS のコードで「情報なし」に差し替えます。この検証は無効にできません。
- 判定と本文の間に規則が破られたら、`final` で `VIOLATION_DETECTED` になり、本文は最終のものに差し替わります。**`final` が届いたら、それまでの本文を `final` の本文に置き換えてください。**
- 回答の URL は、根拠の記事の本文にある URL か、出典と同じサイトの URL だけが残ります。
- 確認期限切れの記事も根拠に使います。出典の `reviewExpired` が true なら、そのことを画面に出すとよいでしょう。
- `truncated` が true なら、長さの上限で打ち切っています。

## 回数の制限

| 単位 | 既定 | 超えたとき |
|---|---|---|
| キーごと | 1分60回 | 429 `answer_rate_limited` |
| 訪問者ごと | 1分10回、1日100回 | 429 `visitor_rate_limited` |

値はサーバーの設定 [`[answer]`](../operator/config.md#回答apianswer) で決まります。配信 API の回数の制限とは別に数えます。エラーの一覧は [エラーコード](../reference/error-codes.md#graphql-のエラー回答api) にあります。

## プライバシーについて

訪問者の質問文は（メールアドレス、電話番号、IP アドレスなどの決まった形を伏せた上で）LLM の接続先に送られます。**氏名や住所のような自由な書き方は伏せられません。** サイトのプライバシーポリシーに書いてください。機密性の高いサイトには、ローカルの LLM を勧めます（[LLMの設定](../operator/llm.md)）。
