# 更新

## 版の決まり

版の番号は、配布物のすべて（サーバー、CLI、Nuxt モジュール）で同じです。セキュリティの修正は最新のマイナー版にだけ出します（`SECURITY.md`）。リリースのノートで変更点を確かめてください。

## 更新の手順

1. **先にバックアップを取ります**（[バックアップと復旧](backup.md)）。元の版に戻す手段はバックアップだけです。
2. 新しいタグを取り出します。

   ```bash
   git fetch --tags && git checkout v1.0.1
   ```

3. ビルドして起動し直します。`deploy` で実行します。

   ```bash
   docker compose build && docker compose up -d
   ```

   配布のイメージを使っているなら、[`docker-compose.override.yml`](install.md#公式の-compose-で配布のイメージを使う) の版を変えて `docker compose pull yomi` の後に `docker compose up -d` します。

4. 確かめます。

   ```bash
   curl http://127.0.0.1:8080/healthz
   ```

   ```bash
   docker compose logs --tail 50 yomi
   ```

5. 管理画面のワークスペースの一覧で、「移行待ち」がないことを確かめます。
6. CLI と Nuxt モジュールも同じ版に入れ替えて、サイトをビルドし直します（[開発者向け](../developer/getting-started.md)）。

サーバーは起動のときに共通の DB の移行を自動で行います。v1.0 は1つのプロセスで動かす前提なので、更新の間は短く止まります。

## 変換器の更新

新しい版で本文の変換が変わると、起動の後に本文の再変換が順に走ることがあります。古い版でも配信は続きます。埋め込みの作り直しがあるときは、ワークスペースの「埋め込みの作成」の予算の範囲で作ります。

## PostgreSQL の更新

公式のイメージは PostgreSQL 18 です（v1.0.0-rc.4 までは 16）。CMS が対応する最低の版は 16 なので、自分で用意した PostgreSQL 16・17 もそのまま使えます。

メジャーバージョンを上げるときは、新しいボリュームへ復元します。ボリュームをそのまま新しいメジャーバージョンで開くことはできません。公式のイメージのメジャーバージョンは自動では上げません（Dependabot でも上げない設定です）。

### PostgreSQL 16 から 18 に移る

v1.0.0-rc.4 までの公式の Docker Compose（PostgreSQL 16、ボリューム `pgdata`）から移る手順です。18 のイメージはデータの置き場所が変わったため、新しい版の Compose は別のボリューム `pgdata18` を使います。古いボリュームは消さないので、うまくいかなければ前の版に戻せます。

1. **前の版のまま**、CMS を止めて、DB の中身をすべて書き出します。

   ```bash
   docker compose stop yomi
   docker compose exec -T db pg_dumpall -U postgres > yomi-pg16.sql
   ```

   書き出したファイルには、DB のロールのパスワードのハッシュと記事の内容が入ります。作業が終わったら安全に消してください。
2. 新しい版のファイル（`deploy/` 一式）に替え、DB だけを起動します。新しいボリュームに空の PostgreSQL 18 ができます（`./data/db-init` が残っていれば、ロールと DB も作られます）。

   ```bash
   docker compose up -d --build --wait db
   ```
3. 書き出したものを読み込みます。`already exists` の行は、すでにあるロールや DB を作ろうとしたもので、問題ありません。

   ```bash
   docker compose exec -T db psql -U postgres -q -f - < yomi-pg16.sql
   ```
4. CMS を起動し、管理画面で記事とワークスペースがそろっていることを確かめます。

   ```bash
   docker compose up -d
   ```
5. 確かめたら、古いボリュームを消します（`docker volume ls` で名前を確かめてから。例: `docker volume rm yomi_pgdata`）。
