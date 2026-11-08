# 公式の Docker Compose

CMSのサーバーと、拡張（pgvector・pg_bigm）を入れた PostgreSQL 16 を起動します（`docs/decisions/12-dev-environment.md`、`13-security.md`「安全な初期設定」）。

```sh
cd deploy
./init.sh https://yomi.example.com      # 最初に1回だけ。設定・秘密の値・暗号化の鍵を ./data に作る
docker compose up -d
docker compose exec yomi yomi-server create-server-admin --config /data/config.toml --email you@example.com
```

- **PostgreSQL はホストにポートを開けません。** CMS とだけ、内部のネットワークでつながります。
- **CMS はホストの `127.0.0.1:8080` だけで待ち受けます。** HTTPS は、前に置くリバースプロキシ（Caddy・nginx など）で行ってください。本番の設定では、公開URLが `https://` でないと起動しません。
  - リバースプロキシから見た接続元を正しく扱うため、プロキシのアドレス（コンテナから見たもの。例: Docker のブリッジのゲートウェイ）を `./data/yomi/config.toml` の `trusted_proxies` に書いてください。
- 管理画面は `https://<公開URL>/admin/` です。イメージの中のファイル（`/usr/share/yomi-admin`）を、CMS のサーバーが配信します（設定の `[admin] dist_dir`）。
- 秘密の値（DBのパスワード）に初期値はありません。`init.sh` が `yomi-server init` で生成します。
- **暗号化の鍵（`./data/yomi/encryption.key`）は、DBのバックアップとは別の場所に保管してください。** 鍵がないとサーバーは起動せず、保存した秘密の値も読めなくなります。
- `./data/db-init/10-yomi.sql` には、初回の起動でロールを作るためのDBのパスワードが入っています。初回の起動の後は消してかまいません。

## 作られるもの

| 場所 | 内容 |
|---|---|
| `./data/yomi/config.toml` | CMSの設定（DBのパスワードを含む） |
| `./data/yomi/encryption.key` | 暗号化の鍵 |
| `./data/yomi/media/` | メディアのファイル |
| `./data/postgres-password` | PostgreSQL のスーパーユーザーのパスワード（CMSは使わない） |
| `./data/db-init/10-yomi.sql` | ロール（`yomi_app`・`yomi_migrate`）・DB・拡張の作成 |
| ボリューム `pgdata` | PostgreSQL のデータ |

## 開発用（擬似LLM）

開発では、`init.sh --dev` で初期化し、`docker-compose.dev.yml` を重ねて起動すると、擬似LLM（`tools/mock-llm`）も内部のネットワークで起動し、CMS の埋め込みの接続先になります。本番では使いません。

```sh
./init.sh --dev https://yomi.example.test
docker compose -f docker-compose.yml -f docker-compose.dev.yml up -d
docker compose -f docker-compose.yml -f docker-compose.dev.yml exec yomi yomi-server llm-check --config /data/config.toml
```

- `--dev` は、設定ファイルに `[llm]`（`http://mock-llm:8080/v1`、`mock-embed-64`）と、外部への送信の内部宛先の例外 `mock-llm` を書きます。

## LLM（埋め込み）

`./data/yomi/config.toml` に `[llm]` の節を書くと、その接続先で記事の埋め込みを作ります（なければ作りません）。管理画面からは変えられません。

```toml
[llm]
base_url = "https://api.openai.com/v1"   # OpenAI互換のAPI。https だけ（例外の内部宛先だけ http も可）
api_key = "..."                          # 必須
embedding_model = "text-embedding-3-small"
embedding_dimensions = 1536              # 1〜2000
# embedding_price_per_million_tokens = 0.02   # 任意。見積もりで費用も出す
```

- ローカルの LLM（Ollama など）は、そのホストを `[egress] internal_destination_exceptions` に入れてください。
- 書いたら `yomi-server llm-check --config /data/config.toml` で、届くかを確かめられます（結果と状態コードだけを出し、相手の応答の本文は出しません）。
