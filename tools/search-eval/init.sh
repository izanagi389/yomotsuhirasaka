#!/usr/bin/env bash
# 評価用の CMS を、指定した埋め込みモデルで作り直して起動する（README.md）。
#
# 使い方: tools/search-eval/init.sh <埋め込みモデル> <次元数>
#   例: tools/search-eval/init.sh bge-m3 1024
#
# - ./work を消して作り直す（前の評価のデータは残らない）
# - 設定ファイルの作り方は deploy/init.sh と同じ（yomi-server init）。違うのは次の3つだけ
#   - LLM の接続先をホストの Ollama にし、外部への送信の内部宛先の例外に入れる
#   - 埋め込みの予算の既定値を大きくする（手元の Ollama なので費用はかからない）
#   - DB は tmpfs（compose.yml）
set -euo pipefail

MODEL="${1:?usage: init.sh <embedding model> <dimensions>}"
DIMENSIONS="${2:?usage: init.sh <embedding model> <dimensions>}"
OLLAMA_HOST_PORT="host.docker.internal:11434"
# 手元の Ollama で、5,000件ほどの文書を何度か埋め込める量
EMBEDDING_TOKENS=100000000

HERE="$(cd "$(dirname "$0")" && pwd)"
cd "$HERE"
WORK="$HERE/work"

docker compose down --remove-orphans >/dev/null 2>&1 || true
rm -rf "$WORK"
umask 077
mkdir -p "$WORK/yomi/media" "$WORK/db-init"
printf 'YOMI_UID=%s\nYOMI_GID=%s\n' "$(id -u)" "$(id -g)" > "$HERE/.env"

# Ollama はキーを確かめないが、設定にはキーが要る。キーは引数に載せず標準入力で渡す
printf 'local\n' | docker compose run --rm --no-deps -T --user "$(id -u):$(id -g)" yomi init \
  --config /data/config.toml \
  --key-file /data/encryption.key \
  --public-url "https://yomi-eval.localhost" \
  --database-url "postgres://yomi_app@db:5432/yomi?sslmode=disable" \
  --migration-database-url "postgres://yomi_migrate@db:5432/yomi?sslmode=disable" \
  --listen "0.0.0.0:8080" \
  --media-root /data/media \
  --llm-base-url "http://${OLLAMA_HOST_PORT}/v1" \
  --llm-embedding-model "$MODEL" \
  --llm-embedding-dimensions "$DIMENSIONS" \
  --llm-api-key-stdin \
  --internal-destination-exception "$OLLAMA_HOST_PORT" >/dev/null

# [llm] の直後に、埋め込みの予算の既定値を足す
CONFIG="$WORK/yomi/config.toml"
awk -v tokens="$EMBEDDING_TOKENS" '
  { print }
  $0 == "[llm]" {
    print "default_workspace_embedding_tokens_per_month = " tokens
    print "default_workspace_embedding_tokens_per_day = " tokens
  }
' "$CONFIG" > "$CONFIG.new"
mv "$CONFIG.new" "$CONFIG"
grep -q '^default_workspace_embedding_tokens_per_month' "$CONFIG" || { echo "init.sh: [llm] not found in the generated config" >&2; exit 1; }

password_of() {
  awk -v section="[$1]" '
    /^\[/ { inside = ($0 == section) }
    inside && /^password = / { sub(/^password = "/, ""); sub(/"$/, ""); print; exit }
  ' "$CONFIG"
}
APP_PW="$(password_of database)"
MIG_PW="$(password_of migration_database)"
if [ -z "$APP_PW" ] || [ -z "$MIG_PW" ] || [[ "$APP_PW$MIG_PW" == *"'"* ]]; then
  echo "init.sh: cannot read the generated database passwords" >&2
  exit 1
fi
head -c 32 /dev/urandom | base64 | tr -d '/+=\n' > "$WORK/postgres-password"
cat > "$WORK/db-init/10-yomi.sql" <<SQL
CREATE ROLE yomi_migrate LOGIN CREATEROLE PASSWORD '${MIG_PW}';
CREATE ROLE yomi_app LOGIN PASSWORD '${APP_PW}';
CREATE DATABASE yomi OWNER yomi_migrate;
GRANT CONNECT ON DATABASE yomi TO yomi_app;
\\connect yomi
CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pg_bigm;
SQL
chmod 755 "$WORK" "$WORK/db-init"
chmod 644 "$WORK/db-init/10-yomi.sql" "$WORK/postgres-password"

docker compose up -d --wait
docker compose exec -T yomi yomi-server llm-check --config /data/config.toml
echo "ready: model=$MODEL dimensions=$DIMENSIONS url=http://127.0.0.1:${YOMI_EVAL_PORT:-8090}"
