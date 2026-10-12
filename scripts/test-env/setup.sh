#!/usr/bin/env bash
# 約束のテストのためのDBと設定を用意する（CIとローカルで共通）。
#
# 使い方: scripts/test-env/setup.sh <PG16のスーパーユーザーのURL> <PG15のスーパーユーザーのURL または ""> <出力先のディレクトリ>
#
# - PG16 に、普段用のロール cms_app と、移行・ロールの作成用のロール cms_migrate（CREATEROLE）と、DB cms を作る
# - cms-server init で、設定ファイルと暗号化の鍵を作り、生成されたパスワードをロールに設定する
# - PG15 にも同じロールとDBを作る（起動を拒否されることの確認用）
# - テストに渡す環境変数を <出力先>/test.env に書き出す
set -euo pipefail

PG16_URL="$1"
PG15_URL="${2:-}"
OUT="$3"
BIN="${CMS_SERVER_BIN:-$(pwd)/target/debug/cms-server}"

mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
rm -f "$OUT/config.toml" "$OUT/encryption.key" "$OUT/test.env"

host_port() { python3 -c 'import sys,urllib.parse as u; p=u.urlsplit(sys.argv[1]); print(f"{p.hostname}:{p.port or 5432}")' "$1"; }
random_pw() { python3 -c 'import secrets; print(secrets.token_urlsafe(24))'; }

# ロールとDBを作り直す。パスワードは標準入力で渡し、コマンドの引数に載せない。
create_roles() {
  local url="$1" app_pw="$2" mig_pw="$3"
  psql "$url" -v ON_ERROR_STOP=1 -q <<SQL
DROP DATABASE IF EXISTS cms;
DROP ROLE IF EXISTS cms_app;
DROP ROLE IF EXISTS cms_migrate;
CREATE ROLE cms_migrate LOGIN CREATEROLE PASSWORD '${mig_pw}';
CREATE ROLE cms_app LOGIN PASSWORD '${app_pw}';
CREATE DATABASE cms OWNER cms_migrate;
GRANT CONNECT ON DATABASE cms TO cms_app;
SQL
}

HP16="$(host_port "$PG16_URL")"
"$BIN" init \
  --config "$OUT/config.toml" \
  --key-file "$OUT/encryption.key" \
  --public-url "https://cms.inv.test" \
  --database-url "postgres://cms_app@${HP16}/cms" \
  --migration-database-url "postgres://cms_migrate@${HP16}/cms" >/dev/null

# テストの舞台に多要素認証のない所属者（編集者など）を作るため、全員への多要素認証の必須（Q197）を無効にする
python3 - "$OUT/config.toml" <<'PY'
import re, sys
path = sys.argv[1]
text = open(path).read()
new, n = re.subn(r"(?m)^mfa_required_for_everyone = true$", "mfa_required_for_everyone = false", text)
if n != 1:
    sys.exit("setup.sh: mfa_required_for_everyone not found in the generated config")
open(path, "w").write(new)
PY

APP_PW="$(python3 -c 'import sys,tomllib; print(tomllib.load(open(sys.argv[1],"rb"))["database"]["password"])' "$OUT/config.toml")"
MIG_PW="$(python3 -c 'import sys,tomllib; print(tomllib.load(open(sys.argv[1],"rb"))["migration_database"]["password"])' "$OUT/config.toml")"
create_roles "$PG16_URL" "$APP_PW" "$MIG_PW"

{
  echo "export CMS_TEST_SERVER_BIN='$BIN'"
  echo "export CMS_TEST_SERVER_KEY_FILE='$OUT/encryption.key'"
  echo "export CMS_TEST_SERVER_DB_URL='postgres://cms_app@${HP16}/cms'"
  echo "export CMS_TEST_SERVER_DB_PASSWORD='$APP_PW'"
  echo "export CMS_TEST_SERVER_MIGRATION_DB_URL='postgres://cms_migrate@${HP16}/cms'"
  echo "export CMS_TEST_SERVER_MIGRATION_DB_PASSWORD='$MIG_PW'"
  echo "export CMS_TEST_PUBLIC_URL='https://cms.inv.test'"
  echo "export CMS_TEST_DB_SUPERUSER_URL='${PG16_URL}'"
  echo "export CMS_TEST_DB_APP_URL='postgres://cms_app:${APP_PW}@${HP16}/cms'"
} > "$OUT/test.env"

if [ -n "$PG15_URL" ]; then
  HP15="$(host_port "$PG15_URL")"
  APP15="$(random_pw)"
  MIG15="$(random_pw)"
  create_roles "$PG15_URL" "$APP15" "$MIG15"
  {
    echo "export CMS_TEST_PG15_URL='postgres://cms_app:${APP15}@${HP15}/cms'"
    echo "export CMS_TEST_PG15_MIGRATION_URL='postgres://cms_migrate:${MIG15}@${HP15}/cms'"
  } >> "$OUT/test.env"
fi
chmod 600 "$OUT/test.env"
echo "Wrote $OUT/test.env"
