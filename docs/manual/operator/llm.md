# LLMの設定

埋め込みも回答の生成も、外部の OpenAI 互換の API を使います。CMS のプロセスの中でモデルは動かしません。OpenAI のほか、Ollama、llama.cpp、vLLM、LM Studio などのローカルの実行も同じ口で使えます。

`[llm]` を書かなければ、埋め込みも回答も作りません（キーワード検索だけで動きます）。管理画面からは変えられません。

## 接続先

```toml
[llm]
base_url = "https://api.openai.com/v1"     # https だけ（内部宛先の例外だけ http も可）
api_key = "…"                              # 必須
embedding_model = "text-embedding-3-small"
embedding_dimensions = 1536                # 1〜2000
chat_model = "…"                           # 回答の生成のモデル。なければ回答 API と AI 支援は使えません
# 単価（任意。100万トークンあたり）。あれば見積もりと画面に費用も出します
# embedding_price_per_million_tokens = 0.02
# chat_input_price_per_million_tokens = 0.5
# chat_output_price_per_million_tokens = 1.5
```

書いたら再起動して、届くかを確かめます（結果と状態コードだけを出し、相手の応答の本文は出しません）。

```bash
docker compose exec yomi yomi-server llm-check --config /data/config.toml
```

## 予算の天井の既定値

書かなければ 0 で、**LLM を呼びません。** ワークスペースごとの天井は [管理画面で決めます](workspaces.md#予算の天井)。トークン数で、UTC の暦の月と日で数えます。

```toml
[llm]
default_workspace_embedding_tokens_per_month = 1000000
default_workspace_embedding_tokens_per_day = 100000
default_workspace_answer_input_tokens_per_month = 1000000
default_workspace_answer_input_tokens_per_day = 100000
default_workspace_answer_output_tokens_per_month = 200000
default_workspace_answer_output_tokens_per_day = 20000
default_workspace_assist_input_tokens_per_month = 500000
default_workspace_assist_input_tokens_per_day = 50000
default_workspace_assist_output_tokens_per_month = 100000
default_workspace_assist_output_tokens_per_day = 10000
```

上の数字は書き方の例です。料金とサイトの規模に合わせて決めてください。使用量を返さない実装では、文字数から多めに推定して数えます。

## ローカルのLLM

**機密性の高いサイトには、ローカルの LLM を勧めます。** 外部の提供会社に次のものが送られるためです。

| 機能 | 送られるもの |
|---|---|
| 埋め込みの作成 | 検索の対象のフィールドの内容 |
| 回答 | 訪問者の質問文（マスクした上で）と、根拠にする記事の断片 |
| 編集者向けの支援 | **公開前の下書き**、記事の断片、答えられなかった質問 |

ローカルの LLM のホストは内部のアドレスなので、そのままでは届きません。例外に入れてください。

```toml
[egress]
internal_destination_exceptions = ["ollama.internal:11434"]

[llm]
base_url = "http://ollama.internal:11434/v1"
api_key = "local"
```

### マスクの限界

訪問者の質問文は、送る前にメールアドレス、電話番号、IP アドレスなどの決まった形を伏せます。**氏名や住所のような自由な書き方は伏せられません。** ワークスペースごとに「拒否する語」を決めることはできますが、すべての個人情報を止めるものではありません。チャットボットを公開するサイトは、プライバシーポリシーに書いてください。

## ワークスペースごとの接続先

ワークスペースが使える接続先とモデルを、設定ファイルで許可します。ワークスペースは名前で選ぶだけで、URL は書けません（内部のサーバーへ送らせる攻撃を防ぐためです）。

```toml
[[llm.allowed_endpoints]]
name = "OpenAI"
base_url = "https://api.openai.com/v1"

[[llm.allowed_embedding_models]]
model = "text-embedding-3-small"
dimensions = 1536
embedding_price_per_million_tokens = 0.02
```

上書きを許すかどうかは、ワークスペースごとに、「予算」の「このワークスペースに、LLMの接続設定の上書きを許す（サーバー管理者）」で決めます。API キーはワークスペースが設定し、DB に暗号化して保存します。保存した後は表示されません。

## 埋め込みのモデルを変える

1. 切り替え先を設定に書いて再起動します。書いただけでは何も起きません。

   ```toml
   [llm.next_embedding]
   model = "text-embedding-3-large"
   dimensions = 1024
   # 接続先を変えるときは base_url と api_key の両方を書きます（今のキーを別の宛先に送らないため）
   ```

2. 「サーバーの管理」→「埋め込みモデル」で、ワークスペースごとのチャンクの数、トークン数、費用の見積もりを確かめて「切り替えを始める」を押します。
3. 埋め込みは各ワークスペースの予算の範囲で作ります。足りなければ一時停止し、そろったワークスペースから順に切り替わります。切り替わるまで、検索は今のモデルで続きます。
4. すべて切り替わったら、`embedding_model` と `embedding_dimensions` を新しい値にして、`[llm.next_embedding]` を消します。

次元数は1〜2000です（pgvector の索引の上限）。応答のベクトルの長さと照合するので、モデルの返す次元数に合わせてください。
