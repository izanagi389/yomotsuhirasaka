#!/usr/bin/env bash
# 公式の Docker Compose の初期化（13-security.md「安全な初期設定」、Q168, Q172, Q173）。最初に1回だけ実行する。
#
# 使い方: deploy/init.sh [--dev] https://cms.example.com
#
# --dev: 開発用（docker-compose.dev.yml を重ねて使う）。擬似LLM（mock-llm）を LLM の接続先にし、
#        外部への送信の内部宛先の例外に mock-llm を入れる（Q139）。本番では付けない。
#
# - cms-server init で、設定ファイル（秘密の値を含む）と暗号化の鍵を ./data/cms に作る（秘密の値に初期値はない）
# - PostgreSQL の初回の起動で、普段用のロール cms_app・移行用のロール cms_migrate（CREATEROLE、スーパーユーザーでない）・
#   DB cms・拡張（pgvector・pg_bigm）を作るSQLを ./data/db-init に書き出す
# - PostgreSQL のスーパーユーザーのパスワードをランダムに作る（CMSは使わない）
#
# 暗号化の鍵（./data/cms/encryption.key）は、DBのバックアップとは別の場所に保管すること。
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
if [ -e "$DATA/cms/config.toml" ] || [ -e "$DATA/db-init" ]; then
  echo "refusing to overwrite an existing ./data (already initialized)" >&2
  exit 1
fi

umask 077
mkdir -p "$DATA/cms/media" "$DATA/db-init"
chmod 700 "$DATA"
# コンテナの CMS は、この利用者で動かす（./data/cms をホストの利用者のまま読み書きできるように）
printf 'CMS_UID=%s\nCMS_GID=%s\n' "$(id -u)" "$(id -g)" > "$HERE/.env"

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
head -c 24 /dev/urandom | base64 | tr -d '/+=\n' | docker compose run --rm --no-deps -T --user "$(id -u):$(id -g)" cms init \
  --config /data/config.toml \
  --key-file /data/encryption.key \
  --public-url "$PUBLIC_URL" \
  --database-url "postgres://cms_app@db:5432/cms" \
  --migration-database-url "postgres://cms_migrate@db:5432/cms" \
  --listen "0.0.0.0:8080" \
  --media-root /data/media \
  --admin-dist-dir /usr/share/cms-admin \
  ${DEV_ARGS[@]+"${DEV_ARGS[@]}"}

# 設定ファイルから、生成されたDBのパスワードを読む（[database] と [migration_database] の password）
password_of() {
  awk -v section="[$1]" '
    /^\[/ { inside = ($0 == section) }
    inside && /^password = / { sub(/^password = "/, ""); sub(/"$/, ""); print; exit }
  ' "$DATA/cms/config.toml"
}
APP_PW="$(password_of database)"
MIG_PW="$(password_of migration_database)"
if [ -z "$APP_PW" ] || [ -z "$MIG_PW" ] || [[ "$APP_PW$MIG_PW" == *"'"* ]]; then
  echo "cannot read the generated database passwords from ./data/cms/config.toml" >&2
  exit 1
fi

# PostgreSQL のコンテナ（postgres の利用者）が読めるようにする。./data 自体はこの利用者だけが開ける
head -c 32 /dev/urandom | base64 | tr -d '/+=\n' > "$DATA/postgres-password"
cat > "$DATA/db-init/10-cms.sql" <<SQL
CREATE ROLE cms_migrate LOGIN CREATEROLE PASSWORD '${MIG_PW}';
CREATE ROLE cms_app LOGIN PASSWORD '${APP_PW}';
CREATE DATABASE cms OWNER cms_migrate;
GRANT CONNECT ON DATABASE cms TO cms_app;
\\connect cms
CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pg_bigm;
SQL
chmod 755 "$DATA/db-init"
chmod 644 "$DATA/db-init/10-cms.sql" "$DATA/postgres-password"

cat <<MSG
Initialized ./data.

Next:
  1. docker compose up -d
  2. docker compose exec cms cms-server create-server-admin --config /data/config.toml --email you@example.com
  3. Put an HTTPS reverse proxy in front of http://127.0.0.1:8080 for ${PUBLIC_URL}
     and add its address (as seen from the container) to trusted_proxies in ./data/cms/config.toml.

Keep ./data/cms/encryption.key somewhere other than your database backups.
After the first start you may delete ./data/db-init/10-cms.sql (it contains the database passwords).
MSG
