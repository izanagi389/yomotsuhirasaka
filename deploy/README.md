# 公式の Docker Compose

CMSのサーバーと、拡張（pgvector・pg_bigm）を入れた PostgreSQL 18 を起動します（CMS が対応する最低の版は 16 です）。

PostgreSQL 16 の公式のイメージ（v1.0.0-rc.4 まで）から移るときは、[更新の手順](../docs/manual/operator/update.md#postgresql-16-から-18-に移る) を見てください。

手順は手順書の [導入](../docs/manual/operator/install.md) にあります。設定は [設定ファイル](../docs/manual/operator/config.md)、LLM は [LLMの設定](../docs/manual/operator/llm.md) を見てください。

```sh
cd deploy
./init.sh https://yomi.example.com      # 最初に1回だけ。設定・秘密の値・暗号化の鍵を ./data に作る
docker compose up -d
docker compose exec yomi yomi-server create-server-admin --config /data/config.toml --email you@example.com
```

開発用（擬似LLM）は `./init.sh --dev` で初期化し、`docker-compose.dev.yml` を重ねて起動します。本番では使いません。
