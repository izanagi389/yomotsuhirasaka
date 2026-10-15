# 公式の Docker Compose

CMSのサーバーと、拡張（pgvector・pg_bigm）を入れた PostgreSQL 16 を起動します（`docs/decisions/12-dev-environment.md`、`13-security.md`「安全な初期設定」）。

```sh
cd deploy
./init.sh https://cms.example.com      # 最初に1回だけ。設定・秘密の値・暗号化の鍵を ./data に作る
docker compose up -d
docker compose exec cms cms-server create-server-admin --config /data/config.toml --email you@example.com
```

- **PostgreSQL はホストにポートを開けません。** CMS とだけ、内部のネットワークでつながります。
- **CMS はホストの `127.0.0.1:8080` だけで待ち受けます。** HTTPS は、前に置くリバースプロキシ（Caddy・nginx など）で行ってください。本番の設定では、公開URLが `https://` でないと起動しません。
  - リバースプロキシから見た接続元を正しく扱うため、プロキシのアドレス（コンテナから見たもの。例: Docker のブリッジのゲートウェイ）を `./data/cms/config.toml` の `trusted_proxies` に書いてください。
- 管理画面は `https://<公開URL>/admin/` です。イメージの中のファイル（`/usr/share/cms-admin`）を、CMS のサーバーが配信します（設定の `[admin] dist_dir`）。
- 秘密の値（DBのパスワード）に初期値はありません。`init.sh` が `cms-server init` で生成します。
- **暗号化の鍵（`./data/cms/encryption.key`）は、DBのバックアップとは別の場所に保管してください。** 鍵がないとサーバーは起動せず、保存した秘密の値も読めなくなります。
- `./data/db-init/10-cms.sql` には、初回の起動でロールを作るためのDBのパスワードが入っています。初回の起動の後は消してかまいません。

## 作られるもの

| 場所 | 内容 |
|---|---|
| `./data/cms/config.toml` | CMSの設定（DBのパスワードを含む） |
| `./data/cms/encryption.key` | 暗号化の鍵 |
| `./data/cms/media/` | メディアのファイル |
| `./data/postgres-password` | PostgreSQL のスーパーユーザーのパスワード（CMSは使わない） |
| `./data/db-init/10-cms.sql` | ロール（`cms_app`・`cms_migrate`）・DB・拡張の作成 |
| ボリューム `pgdata` | PostgreSQL のデータ |
