# 擬似LLM（mock-llm）

OpenAI互換の埋め込みとチャットだけを持つ、開発とテスト用の小さなサーバーです（`docs/decisions/12-dev-environment.md`）。CMSからは普通の接続先として見えます。本番では使いません。依存はありません（Node.js の標準の機能だけで動きます）。

```sh
npm run build -w @yomotsuhirasaka/mock-llm
PORT=8080 npm start -w @yomotsuhirasaka/mock-llm     # 既定は PORT=8080・HOST=0.0.0.0
```

- コンテナ: `deploy/mock-llm.Dockerfile`（開発用の `deploy/docker-compose.dev.yml` で起動）
- 約束のテスト: ハーネス（`tests/invariants/support/global-setup.ts`）が `dist/main.js` を別のプロセスとして起動し、`tests/invariants/support/mock-llm.ts` から操作します。

## API

どれも `Authorization: Bearer <キー>` が要ります（なければ 401）。キーの中身は空でなければ何でも受け付けます。**障害の設定・回答の組み立て方・受け取ったリクエストの記録は、キーごとに分かれます。**

| API | 内容 |
|---|---|
| `POST /v1/embeddings` | モデル名 `mock-embed-<次元数>`（1〜4096。例: `mock-embed-64`）。文字の bigram をハッシュして次元に振り分け、長さ 1 に正規化したベクトルを返す。同じ入力には同じ値、言葉が重なる文どうしは似たベクトルになる |
| `POST /v1/chat/completions` | モデル名 `mock-chat`。`stream: true` なら SSE（`data: …`、最後に `data: [DONE]`。`stream_options.include_usage` で usage のチャンク） |

- 知らないモデル名は 404、知らないパスは 404。
- `usage` のトークン数は、入力ごとに `ceil(文字数 ÷ 4)`（少なくとも 1。文字数は Unicode のコードポイント）。チャットの出力も同じ規則。

### チャットの回答の形

プロンプトと回答の形は `src/format.json` にあります。CMSも同じファイルを読みます（情報源は1か所）。

- プロンプトの中の断片は `[n] 本文` の行。質問は、最後の `user` のメッセージから断片の行を除いたもの。
- 回答の1行目は判定の先行宣言（`ANSWERABLE: yes; SOURCES: 1,3` または `ANSWERABLE: no; SOURCES: none`）、2行目からが本文。本文では `[n]` で出典を示す。

組み立て方（キーごとに選べる。どれも同じ入力には同じ回答を返す）:

| 組み立て方 | 内容 |
|---|---|
| `overlap`（既定） | 質問との bigram の重なりが多い順に、最大 `maxSources`（既定 2）個の断片を出典にする。重なりが `minShared`（既定 2）未満の断片は使わない。使える断片がなければ「答えられない」 |
| `firstN` | 番号の小さい順に、最大 `n`（既定 2）個を出典にする |
| `fixed` | テストが決めた回答（宣言の行を含む全体）をそのまま返す。回答の中身を変える障害は効かない |

本文は、出典にした断片の最初の1文に `[n]` を付けてつないだもの。

## テストからの操作（`/_mock/`）

| API | 内容 |
|---|---|
| `PUT /_mock/faults` | 障害を設定する（`{"faults": [...]}`。前の設定は置き換わる） |
| `DELETE /_mock/faults` | 障害を消す |
| `PUT /_mock/strategy` | 回答の組み立て方を決める（`{"strategy": {"kind": "overlap" \| "firstN" \| "fixed", ...}}`） |
| `DELETE /_mock/strategy` | 既定（`overlap`）に戻す |
| `GET /_mock/requests` | 受け取ったリクエスト（古い順。本文・効いた障害・状態コード。直近の 1000 件） |
| `DELETE /_mock/requests` | 記録を消す |

記録はメモリの中だけに持ち、ファイルには書きません。

### 障害

`{"kind": ..., "endpoint": "embeddings" | "chat", "count": n, ...}`。`endpoint` を省くと両方に効きます。`count` を省くと消すまで続き、指定すると次の n 回だけ効きます。

| 障害 | 内容 |
|---|---|
| `timeout` | `delayMs`（既定 120000）だけ待ってから応答を始める |
| `rateLimit` | 429。`Retry-After` は `retryAfterSeconds`（既定 1） |
| `serverError` | 500 |
| `brokenJson` | 200 で、壊れた JSON（ストリーミングなら壊れたチャンク）を返す |
| `noUsage` | `usage` を返さない |
| `noCitations` | チャットだけ。本文から引用記号を取り除く（宣言はそのまま） |
| `unknownSource` | チャットだけ。実在しない出典番号を宣言し、本文でも使う |
| `undeclaredCitation` | チャットだけ。宣言していない番号を本文で使う |
| `cutStream` | チャットだけ。応答の途中で接続を切る（`[DONE]` も終わりの理由もない） |
