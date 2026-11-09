# Webhookの受け方と署名の書式

CMS は、記事の状態が変わると、登録された送信先に署名付きの POST を送ります。送信先は「管理」→「Webhook」で登録します（https の URL で、公開のアドレスに解決されるものだけ）。

Nuxt モジュールを使うなら、受け口は [モジュールが持っています](nuxt-module.md#cmsからのwebhookの受け口)。この章は、自分で受けるときの資料です。

## 本文

`Content-Type: application/json` です。**記事の本文や下書きの内容は入っていません。** 必要なら、受けた後に配信 API で取り直してください。

```json
{
  "type": "entry.published",
  "workspaceId": "…",
  "entryId": "…",
  "translationGroupId": "…",
  "locale": "ja",
  "url": "https://example.com/ja/blog/hello"
}
```

| イベント（`type`） | いつ |
|---|---|
| `entry.published` | 初めて公開したとき |
| `entry.updated` | 公開中の記事を更新したとき |
| `entry.unpublished` | 非公開にしたとき |
| `entry.deleted` | 削除したとき |
| `entry.corrected` | 訂正として公開したとき |
| `entry.version_erased` | 版の本文を抹消したとき |
| `entry.settings_changed` | 公開設定を変えたとき |
| `entry.url_changed` | 公開 URL が変わったとき |
| `entry.scheduled` | 予約したとき（`scheduledAt` が付く） |
| `entry.schedule_cancelled` | 予約を取り消したとき（`reason` が付く） |
| `entries.related_changed` | 関連記事の一覧が変わったとき（記事の参照のまとまり。1通に100件まで） |
| `webhook.test` | 管理画面の「テスト送信」 |

`url` は最後に公開したときのものです。一度も公開していなければ付きません。予約版の新しい URL は送りません。受け手は、知らないイベントや知らない項目を無視してください。

## 署名の書式

```
Webhook-Signature: t=<UNIX秒>,id=<イベントID>,v1=<16進>[,v1=<16進>]
```

| 部分 | 内容 |
|---|---|
| `t` | 送信時刻。**試行ごとに付け直します** |
| `id` | イベント ID。**やり直しても変わりません** |
| `v1` | 署名。秘密の入れ替え中は、新旧それぞれの秘密で作ったものが並びます |

署名は HMAC-SHA256 です。

- 鍵: 署名の秘密の文字列（`whsec_` で始まるもの全体）のバイト列
- 対象: `<t>.<id>.<本文>`（`.` でつないだもの。本文は届いたバイト列のまま）
- 値: 結果を小文字の16進にしたもの

## 受け手が確かめること

1. `v1` のどれかが、自分で計算した値と一致すること。比べるときは一定時間の比較を使います。
2. `t` が今から離れすぎていないこと（目安は5分）。
3. `id` をまだ処理していないこと（同じものの再送を断ります）。
4. 確かめてから処理し、**すぐに 2xx を返す**こと。時間のかかる処理は後で行ってください。

本文を JSON として読み直してから署名を計算しないでください。届いたバイト列をそのまま使います。

```js
import { createHmac, timingSafeEqual } from "node:crypto";

/** 署名を確かめる。正しければイベント ID を返す。 */
export function verifyWebhook(header, rawBody, secrets, now = Date.now() / 1000) {
  const parts = header.split(",").map((p) => p.split("="));
  const t = parts.find(([k]) => k === "t")?.[1];
  const id = parts.find(([k]) => k === "id")?.[1];
  const sent = parts.filter(([k]) => k === "v1").map(([, v]) => Buffer.from(v, "hex"));
  if (!t || !id || Math.abs(now - Number(t)) > 300) return null;
  for (const secret of secrets) {
    const mine = createHmac("sha256", secret).update(`${t}.${id}.`).update(rawBody).digest();
    if (sent.some((s) => s.length === mine.length && timingSafeEqual(s, mine))) return id;
  }
  return null;
}
```

## 送信のやり直し

2xx 以外の応答や通信の失敗は、やり直します。間隔は30秒から倍々に延び、最後は1時間おきで、10回まで試します。結果は「管理」→「Webhook」の「送信の記録」で見られます（状態コードだけで、相手の応答の本文は記録しません）。

送信先の条件を満たさないものなど、やり直しても届かない失敗は、やり直しません。リダイレクトはたどりません。

## 秘密の入れ替え

「秘密を入れ替える」を押すと、新しい秘密が一度だけ表示されます。その後24時間は新旧の両方で署名するので、その間に受け手の設定を新しい秘密に変えてください。受け手は、入れ替えの間だけ、両方の秘密で確かめます。

## 監査ログの書き出し先

監査ログの Webhook も同じ署名の書式です。違いは次のとおりです（[監視と監査ログ](../operator/monitoring.md)）。

- 本文は `application/x-ndjson`（1行1件の JSON）で、まとめて届きます。
- `id` は、その中の最初の記録の ID です。
