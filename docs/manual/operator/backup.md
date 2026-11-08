# バックアップと復旧

この手順は、公式の Docker Compose で実際に復旧して確かめています。ただし、**自分の環境でも一度復旧を試してください。** 試していないバックアップは、当てになりません。

コマンドは `deploy` のディレクトリで実行します。

## 何を取るか

| もの | 取り方 | 注意 |
|---|---|---|
| DB | `pg_dumpall` | **`pg_dump` だけでは足りません。** ワークスペースごとの DB のロールは DB の外にあるので、ロールも一緒に取ります |
| メディア | `./data/yomi/media` | ファイルの実体は DB に入っていません |
| 設定 | `./data/yomi/config.toml`、`./data/postgres-password`、`.env` | 秘密の値が入っています |
| **暗号化の鍵** | `./data/yomi/encryption.key` | **DB のバックアップとは別の場所に保管します** |

**鍵がないと、サーバーは起動せず、保存した秘密の値（TOTP の秘密、Webhook の秘密、LLM の API キーなど）も読めません。** 鍵と DB のバックアップを同じ場所に置くと、バックアップが漏れたときに暗号化の意味がなくなります。鍵を入れ替えたら、保管している鍵も新しくしてください。

## 守ること

- **バックアップ自体も暗号化してください。** 記事、利用者のメールアドレス、パスワードのハッシュ、質問ログが入っています。
- **書き換えや削除ができない保管を併用してください**（オフライン、変更できないストレージなど）。ランサムウェアはつながっているバックアップも暗号化します。
- **保存期間を決めてください。** 版の本文を抹消しても、それより前のバックアップには残ります。個人情報の削除依頼などに、どこまで応えられるかに関わります。

## 取る

```bash
mkdir -p backup
```

```bash
docker compose exec -T db pg_dumpall -U postgres | gzip > backup/db.sql.gz
```

```bash
tar -czf backup/media.tar.gz -C data/yomi media
```

```bash
cp data/yomi/config.toml data/postgres-password .env backup/
```

鍵は別の場所へコピーします。

```bash
cp data/yomi/encryption.key /安全な別の場所/
```

できあがった `backup` を暗号化して、サーバーの外へ移します。例えば次のようにします（`gpg` は一例です）。

```bash
tar -cf - backup | gpg --symmetric --cipher-algo AES256 > backup-$(date +%F).tar.gpg
```

DB を先に、メディアを後に取ってください。記事が指すファイルが欠けないようにするためです。定期的に実行し、**失敗に気づける仕組み**を作ってください。

## 復旧する

新しいサーバー、または PostgreSQL のボリュームを消した状態から戻します。**`./init.sh` は実行しません。** 新しい秘密の値を作ってしまいます。

1. リポジトリの同じ版を取り出して、バックアップを復号します。`deploy` のディレクトリに `backup` を置きます。

2. ファイルを戻します。

   ```bash
   mkdir -p data/yomi data/db-init && chmod 700 data && chmod 755 data/db-init
   ```

   ```bash
   cp backup/config.toml /安全な別の場所/encryption.key data/yomi/
   ```

   ```bash
   cp backup/postgres-password data/ && chmod 644 data/postgres-password && cp backup/.env .env
   ```

   ```bash
   tar -xzf backup/media.tar.gz -C data/yomi
   ```

   `.env` はコンテナを動かす利用者の番号です。別の利用者で動かすなら、`YOMI_UID` と `YOMI_GID` を変えて、`data/yomi` の持ち主も合わせてください。

3. DB だけを起動します。`data/db-init` は空のままにします。

   ```bash
   docker compose build && docker compose up -d --wait db
   ```

4. DB を戻します。`role "postgres" already exists` というエラーが1つ出ますが、問題ありません。

   ```bash
   gunzip -c backup/db.sql.gz | docker compose exec -T db psql -U postgres -d postgres -q -o /dev/null
   ```

5. サーバーを起動します。

   ```bash
   docker compose up -d
   ```

6. 確かめます。

   - `curl http://127.0.0.1:8080/healthz` が `ok` を返す
   - 管理画面にログインできる（多要素認証のコードが通れば、鍵が合っています）
   - サイトから記事が読める
   - 画像が表示される
   - ワークスペースの一覧に「移行待ち」がない

バックアップの後に発行されたキーやトークンは使えません。逆に、バックアップの後に失効させたものは有効に戻っています。**事故からの復旧なら、[事故対応](incident.md) に従って入れ替えてください。**

## 取れないもの

- CDN やサイトの側のキャッシュ
- 外に書き出した監査ログ（別に保管してください）
