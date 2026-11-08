# 導入

公式の Docker Compose で動かすのが基本です。CMS のサーバーと、拡張（pgvector、pg_bigm）を入れた PostgreSQL 16 が起動します。

## 用意するもの

- Docker と Docker Compose
- 公開する HTTPS のドメイン（例: `yomi.example.com`）
- HTTPS を受けるリバースプロキシ（Caddy、nginx など）
- できればメディア用の別のドメイン（例: `media.example.com`）と SMTP

## 1. 取得して初期化する

リリースのタグを取り出します。

```bash
git clone https://github.com/izanagi389/yomotsuhirasaka.git
```

```bash
cd yomotsuhirasaka && git checkout v1.0.0
```

最初に1回だけ、初期化します。設定、秘密の値、暗号化の鍵を `deploy/data` に作ります。

```bash
cd deploy && ./init.sh https://yomi.example.com
```

| 場所 | 内容 |
|---|---|
| `./data/yomi/config.toml` | CMS の設定（DB のパスワードを含む） |
| `./data/yomi/encryption.key` | 暗号化の鍵 |
| `./data/yomi/media/` | メディアのファイル |
| `./data/postgres-password` | PostgreSQL のスーパーユーザーのパスワード（CMS は使いません） |
| `./data/db-init/10-yomi.sql` | ロール（`yomi_app`、`yomi_migrate`）、DB、拡張の作成 |
| ボリューム `pgdata` | PostgreSQL のデータ |

- 秘密の値に初期値はありません。`yomi-server init` が生成します。
- **`./data/yomi/encryption.key` は、DB のバックアップとは別の場所にも保管してください。** 鍵がないとサーバーは起動せず、保存した秘密の値も読めなくなります（[バックアップと復旧](backup.md)）。

## 2. 起動する

```bash
docker compose up -d
```

- **PostgreSQL はホストにポートを開けません。** CMS とだけ、内部のネットワークでつながります。
- **CMS はホストの `127.0.0.1:8080` だけで待ち受けます。**

起動を確かめます。`ok` と出れば動いています。

```bash
curl http://127.0.0.1:8080/healthz
```

本番の設定では、弱い秘密の値、スーパーユーザーでの DB の接続、古い PostgreSQL などがあると、理由を表示して起動しません（[起動時の検査](config.md#起動時の検査)）。ログは `docker compose logs yomi` で見られます。

`./data/db-init/10-yomi.sql` には DB のパスワードが入っています。初回の起動の後は消してかまいません。

## 3. リバースプロキシを置く

HTTPS はリバースプロキシで受けて、`http://127.0.0.1:8080` に渡します。本番の設定では、公開 URL が `https://` でないと起動しません。Caddy なら次のとおりです。

```
yomi.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

回答のストリーミング（SSE）を中継するので、nginx では `proxy_buffering off` にしてください。

CMS は `X-Forwarded-For` を、信頼すると書いたプロキシから届いたときだけ使います。ログインの回数の制限などに必要なので、**プロキシのアドレスを設定してください。** 書くのは、コンテナから見たプロキシのアドレスです。ホストでプロキシを動かすなら Docker のブリッジのゲートウェイで、次のコマンドで分かります（`yomi` は Compose のプロジェクト名です）。

```bash
docker network inspect yomi_outside --format '{{(index .IPAM.Config 0).Gateway}}'
```

`./data/yomi/config.toml` の先頭のほうに `trusted_proxies = []` の行があるので、書き換えます。

```toml
trusted_proxies = ["172.18.0.1"]
```

設定を変えたら再起動します。

```bash
docker compose restart yomi
```

## 4. 最初のサーバー管理者を作る

サーバーの上でだけ作れます。パスワードは一度だけ表示されます。

```bash
docker compose exec yomi yomi-server create-server-admin --config /data/config.toml --email you@example.com
```

`https://yomi.example.com/admin/` を開いてログインし、多要素認証を設定します。サーバー管理者は、多要素認証がないと管理の操作ができません。

## 5. 続けて行うこと

- [設定ファイル](config.md) で、メディアのドメイン、SMTP、漏えいしたパスワードの照会を設定する
- テンプレートをデプロイして、ワークスペースを作る（[ワークスペースと利用者](workspaces.md)）
- [バックアップ](backup.md) を組み、復旧を一度試す
- [監視](monitoring.md) を組む

## 署名の確かめ方

配布物には OIDC による鍵なしの署名（Sigstore の cosign）と SBOM（SPDX JSON）が付いています。cosign が要ります。

### コンテナイメージ

イメージは `ghcr.io/izanagi389/yomotsuhirasaka` です（x86_64 と arm64）。

```bash
cosign verify ghcr.io/izanagi389/yomotsuhirasaka:1.0.0 --certificate-identity-regexp '^https://github\.com/izanagi389/yomotsuhirasaka/\.github/workflows/release\.yml@refs/tags/v' --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

SBOM は attestation として付いています。

```bash
cosign verify-attestation --type spdxjson ghcr.io/izanagi389/yomotsuhirasaka:1.0.0 --certificate-identity-regexp '^https://github\.com/izanagi389/yomotsuhirasaka/\.github/workflows/release\.yml@refs/tags/v' --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

### 公式の Compose で配布のイメージを使う

公式の Compose は、既定ではタグのソースからイメージをビルドします。署名を確かめた配布のイメージを使うなら、`deploy/docker-compose.override.yml` を置いてください（`./init.sh` の前に）。

```yaml
services:
  yomi:
    build: !reset null
    image: ghcr.io/izanagi389/yomotsuhirasaka:1.0.0
```

PostgreSQL のイメージは配布していません。`deploy/postgres.Dockerfile` からビルドします。

### バイナリ

GitHub Releases に、`yomotsuhirasaka-<版>-linux-x86_64.tar.gz` と `-linux-aarch64.tar.gz`、それぞれの SBOM、`SHA256SUMS`、ファイルごとの `.sigstore` があります。

```bash
cosign verify-blob --bundle SHA256SUMS.sigstore --certificate-identity-regexp '^https://github\.com/izanagi389/yomotsuhirasaka/\.github/workflows/release\.yml@refs/tags/v' --certificate-oidc-issuer https://token.actions.githubusercontent.com SHA256SUMS
```

```bash
sha256sum --check --ignore-missing SHA256SUMS
```

## バイナリで動かす

Docker を使わないときの手順です。Debian trixie でビルドしているので、同じ世代以降の glibc と libssl3 が要ります。

1. PostgreSQL 16 以上に拡張 pgvector と pg_bigm（または PGroonga）を入れます。
2. `tar.gz` を展開します。中身は `yomi-server` と管理画面のファイル（`admin/`）です。
3. 設定と鍵を作ります。

   ```bash
   ./yomi-server init --config /etc/yomi/config.toml --key-file /etc/yomi/encryption.key --public-url https://yomi.example.com --database-url postgres://yomi_app@127.0.0.1:5432/yomi --migration-database-url postgres://yomi_migrate@127.0.0.1:5432/yomi --media-root /var/lib/yomi/media --admin-dist-dir /opt/yomi/admin
   ```

4. 設定ファイルの `password` の値で、ロールと DB を作ります。スーパーユーザーにはしないでください。

   ```sql
   CREATE ROLE yomi_migrate LOGIN CREATEROLE PASSWORD '<[migration_database] の password>';
   CREATE ROLE yomi_app LOGIN PASSWORD '<[database] の password>';
   CREATE DATABASE yomi OWNER yomi_migrate;
   GRANT CONNECT ON DATABASE yomi TO yomi_app;
   \connect yomi
   CREATE EXTENSION IF NOT EXISTS vector;
   CREATE EXTENSION IF NOT EXISTS pg_bigm;
   ```

5. `./yomi-server serve --config /etc/yomi/config.toml` で起動します。専用の利用者で、systemd などから動かしてください。

既定の待ち受けは `127.0.0.1:8080` です。リバースプロキシからの先は同じです。`trusted_proxies` には `127.0.0.1` を書きます。

## 開発用（擬似LLM）

開発では、`init.sh --dev` で初期化し、`docker-compose.dev.yml` を重ねて起動すると、擬似 LLM（`tools/mock-llm`）も内部のネットワークで起動し、埋め込みの接続先になります。**本番では使いません。**

```bash
./init.sh --dev https://yomi.example.test
```

```bash
docker compose -f docker-compose.yml -f docker-compose.dev.yml up -d
```
