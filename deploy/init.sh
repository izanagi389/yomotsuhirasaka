#!/usr/bin/env bash
# 公式の Docker Compose の初期化（13-security.md「安全な初期設定」、Q168, Q172, Q173）。最初に1回だけ実行する。
#
# 使い方: deploy/init.sh [--dev] https://yomi.example.com
#
# --dev: 開発用（docker-compose.dev.yml を重ねて使う）。擬似LLM（mock-llm）を LLM の接続先にし、
#        外部への送信の内部宛先の例外に mock-llm を入れる（Q139）。本番では付けない。
#
# - yomi-server init で、設定ファイル（秘密の値を含む）と暗号化の鍵を ./data/yomi に作る（秘密の値に初期値はない）
# - PostgreSQL の初回の起動で、普段用のロール yomi_app・移行用のロール yomi_migrate（CREATEROLE、スーパーユーザーでない）・
#   DB yomi・拡張（pgvector・pg_bigm）を作るSQLを ./data/db-init に書き出す
# - PostgreSQL のスーパーユーザーのパスワードをランダムに作る（CMSは使わない）
#
# 暗号化の鍵（./data/yomi/encryption.key）は、DBのバックアップとは別の場所に保管すること。
set -euo pipefail

DEV=0
if [ "${1:-}" = "--dev" ]; then
  DEV=1
  shift
fi
PUBLIC_URL="${1:-}"
if [[ ! "$PUBLIC_URL" =~ ^https:// ]]; then
  echo "usage: deploy/init.sh [--dev] https://<public host of the CMS>" >&2
  exit 2
fi
HERE="$(cd "$(dirname "$0")" && pwd)"
cd "$HERE"
DATA="$HERE/data"
if [ -e "$DATA/yomi/config.toml" ] || [ -e "$DATA/db-init" ]; then
  echo "refusing to overwrite an existing ./data (already initialized)" >&2
  exit 1
fi

umask 077
mkdir -p "$DATA/yomi/media" "$DATA/db-init"
chmod 700 "$DATA"
# コンテナの CMS は、この利用者で動かす（./data/yomi をホストの利用者のまま読み書きできるように）
printf 'YOMI_UID=%s\nYOMI_GID=%s\n' "$(id -u)" "$(id -g)" > "$HERE/.env"

# 擬似LLMはどんなキーでも受け付ける。キーはコマンドの引数に載せず、標準入力で渡す
DEV_ARGS=()
if [ "$DEV" = 1 ]; then
  DEV_ARGS=(
    --llm-base-url "http://mock-llm:8080/v1"
    --llm-embedding-model "mock-embed-64"
    --llm-embedding-dimensions 64
    --llm-api-key-stdin
    --internal-destination-exception mock-llm
  )
fi

docker compose build
head -c 24 /dev/urandom | base64 | tr -d '/+=\n' | docker compose run --rm --no-deps -T --user "$(id -u):$(id -g)" yomi init \
  --config /data/config.toml \
  --key-file /data/encryption.key \
  --public-url "$PUBLIC_URL" \
  --database-url "postgres://yomi_app@db:5432/yomi?sslmode=disable" \
  --migration-database-url "postgres://yomi_migrate@db:5432/yomi?sslmode=disable" \
  --listen "0.0.0.0:8080" \
  --media-root /data/media \
  --admin-dist-dir /usr/share/yomi-admin \
  ${DEV_ARGS[@]+"${DEV_ARGS[@]}"}

# 設定ファイルから、生成されたDBのパスワードを読む（[database] と [migration_database] の password）
password_of() {
  awk -v section="[$1]" '
    /^\[/ { inside = ($0 == section) }
    inside && /^password = / { sub(/^password = "/, ""); sub(/"$/, ""); print; exit }
  ' "$DATA/yomi/config.toml"
}
APP_PW="$(password_of database)"
MIG_PW="$(password_of migration_database)"
if [ -z "$APP_PW" ] || [ -z "$MIG_PW" ] || [[ "$APP_PW$MIG_PW" == *"'"* ]]; then
  echo "cannot read the generated database passwords from ./data/yomi/config.toml" >&2
  exit 1
fi

# PostgreSQL のコンテナ（postgres の利用者）が読めるようにする。./data 自体はこの利用者だけが開ける
head -c 32 /dev/urandom | base64 | tr -d '/+=\n' > "$DATA/postgres-password"
cat > "$DATA/db-init/10-yomi.sql" <<SQL
CREATE ROLE yomi_migrate LOGIN CREATEROLE PASSWORD '${MIG_PW}';
CREATE ROLE yomi_app LOGIN PASSWORD '${APP_PW}';
CREATE DATABASE yomi OWNER yomi_migrate;
GRANT CONNECT ON DATABASE yomi TO yomi_app;
\\connect yomi
CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pg_bigm;
SQL
chmod 755 "$DATA/db-init"
chmod 644 "$DATA/db-init/10-yomi.sql" "$DATA/postgres-password"

cat <<MSG
Initialized ./data.

Next:
  1. docker compose up -d
  2. docker compose exec yomi yomi-server create-server-admin --config /data/config.toml --email you@example.com
  3. Put an HTTPS reverse proxy in front of http://127.0.0.1:8080 for ${PUBLIC_URL}
     and add its address (as seen from the container) to trusted_proxies in ./data/yomi/config.toml.

Keep ./data/yomi/encryption.key somewhere other than your database backups.
After the first start you may delete ./data/db-init/10-yomi.sql (it contains the database passwords).
MSG
