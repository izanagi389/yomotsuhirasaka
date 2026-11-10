#!/usr/bin/env bash
# 約束のテストのためのDBと設定を用意する（CIとローカルで共通）。
#
# 使い方: scripts/test-env/setup.sh <PG16のスーパーユーザーのURL> <PG15のスーパーユーザーのURL または ""> <出力先のディレクトリ>
#
# - PG16 に、普段用のロール yomi_app と、移行・ロールの作成用のロール yomi_migrate（CREATEROLE）と、DB yomi を作る
#   （DB yomi には pgvector の拡張 vector を作る。PG16 には pgvector が入っていること）
# - yomi-server init で、設定ファイルと暗号化の鍵を作り、生成されたパスワードをロールに設定する
# - PG15 にも同じロールとDBを作る（起動を拒否されることの確認用）
# - テストの実行環境（ハーネス）のTLSの証明書（CAと、*.inv.test・127.0.0.1 の証明書）を作る
# - テストに渡す環境変数を <出力先>/test.env に書き出す
#
# Webhookなどの外部への送信のテスト（INV-WH-*）には、`inv.test` のゾーンの名前解決をテストのDNSサーバーに
# 回す設定も要る（scripts/test-env/harness-dns.sh、root で実行）。
set -euo pipefail

PG16_URL="$1"
PG15_URL="${2:-}"
OUT="$3"
BIN="${YOMI_SERVER_BIN:-$(pwd)/target/debug/yomi-server}"

mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
mkdir -p "$OUT/fs"
rm -f "$OUT/config.toml" "$OUT/encryption.key" "$OUT/test.env"

host_port() { python3 -c 'import sys,urllib.parse as u; p=u.urlsplit(sys.argv[1]); print(f"{p.hostname}:{p.port or 5432}")' "$1"; }
with_database() { python3 -c 'import sys,urllib.parse as u; p=u.urlsplit(sys.argv[1]); print(u.urlunsplit(p._replace(path="/"+sys.argv[2])))' "$1" "$2"; }
random_pw() { python3 -c 'import secrets; print(secrets.token_urlsafe(24))'; }

# ロールとDBを作り直す。パスワードは標準入力で渡し、コマンドの引数に載せない。
create_roles() {
  local url="$1" app_pw="$2" mig_pw="$3"
  # 前の実行で driver.createIsolatedInstance が作ったDB（yomi_iso_*）も消す（ロールを作り直せるように）
  local isolated
  isolated="$(psql "$url" -At -c "SELECT datname FROM pg_database WHERE datname LIKE 'yomi\_iso\_%'")"
  for db in $isolated; do psql "$url" -v ON_ERROR_STOP=1 -q -c "DROP DATABASE \"$db\"" ; done
  psql "$url" -v ON_ERROR_STOP=1 -q <<SQL
DROP DATABASE IF EXISTS yomi;
DROP ROLE IF EXISTS yomi_app_audit_purger;
DROP ROLE IF EXISTS yomi_app;
DROP ROLE IF EXISTS yomi_migrate;
CREATE ROLE yomi_migrate LOGIN CREATEROLE PASSWORD '${mig_pw}';
CREATE ROLE yomi_app LOGIN PASSWORD '${app_pw}';
CREATE DATABASE yomi OWNER yomi_migrate;
GRANT CONNECT ON DATABASE yomi TO yomi_app;
SQL
}

HP16="$(host_port "$PG16_URL")"
"$BIN" init \
  --config "$OUT/config.toml" \
  --key-file "$OUT/encryption.key" \
  --public-url "https://yomi.inv.test" \
  --database-url "postgres://yomi_app@${HP16}/yomi" \
  --migration-database-url "postgres://yomi_migrate@${HP16}/yomi" \
  --media-root "$OUT/fs/media" >/dev/null

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
# 検索用データのベクトル（pgvector）。拡張はスーパーユーザーが前もって作る（CMSは作らない。08-search-and-related.md）。
# 日本語のキーワード検索の拡張（pg_bigm）は入れない（拡張がない環境の約束 INV-SRC-05 を確かめるため）
psql "$(with_database "$PG16_URL" yomi)" -v ON_ERROR_STOP=1 -q -c "CREATE EXTENSION IF NOT EXISTS vector"

{
  echo "export YOMI_TEST_SERVER_BIN='$BIN'"
  echo "export YOMI_TEST_SERVER_KEY_FILE='$OUT/encryption.key'"
  echo "export YOMI_TEST_SERVER_DB_URL='postgres://yomi_app@${HP16}/yomi'"
  echo "export YOMI_TEST_SERVER_DB_PASSWORD='$APP_PW'"
  echo "export YOMI_TEST_SERVER_MIGRATION_DB_URL='postgres://yomi_migrate@${HP16}/yomi'"
  echo "export YOMI_TEST_SERVER_MIGRATION_DB_PASSWORD='$MIG_PW'"
  echo "export YOMI_TEST_PUBLIC_URL='https://yomi.inv.test'"
  # スーパーユーザーの接続先は、CMSのDB（yomi）にする（テーブルの一覧や権限を調べるため）
  echo "export YOMI_TEST_DB_SUPERUSER_URL='$(with_database "$PG16_URL" yomi)'"
  echo "export YOMI_TEST_DB_APP_URL='postgres://yomi_app:${APP_PW}@${HP16}/yomi'"
  # 管理画面（ビルドした SPA。ルートで npm ci の後、cd admin && npx nuxt prepare && npm run generate）
  echo "export YOMI_TEST_ADMIN_DIST='$(pwd)/admin/.output/public'"
  # CMSが書くファイル（メディアなど）の置き場所。テストは、この下に想定外のファイルができていないかを調べる
  echo "export YOMI_TEST_FS_ROOT='$OUT/fs'"
} > "$OUT/test.env"

# テストの実行環境（ハーネス）。CMSと同じホストで待ち受ける（tests/invariants/README.md「テストの実行環境」）
# - 例外として許す宛先: 127.0.0.1（ホスト名もIPアドレスで渡す）
# - 例外にない内部のアドレス: 127.0.0.2（ループバックの別のアドレス。Linuxでは追加の設定なしで待ち受けられる）
# - TLS: テスト用のCAと、*.inv.test・127.0.0.1 の証明書。CMSは [egress] extra_ca_files でこのCAを信頼する
HARNESS_DNS_PORT="${YOMI_TEST_HARNESS_DNS_PORT:-5354}"
TLS_DIR="$OUT/harness-tls"
rm -rf "$TLS_DIR"
mkdir -p "$TLS_DIR"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 30 \
  -subj "/CN=yomi invariant tests CA" \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign" \
  -keyout "$TLS_DIR/ca.key" -out "$TLS_DIR/ca.pem" 2>/dev/null
openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -subj "/CN=harness.inv.test" -keyout "$TLS_DIR/harness.key" -out "$TLS_DIR/harness.csr" 2>/dev/null
printf 'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:*.inv.test,DNS:localhost,IP:127.0.0.1\n' > "$TLS_DIR/harness.ext"
openssl x509 -req -in "$TLS_DIR/harness.csr" -CA "$TLS_DIR/ca.pem" -CAkey "$TLS_DIR/ca.key" -CAcreateserial \
  -days 30 -extfile "$TLS_DIR/harness.ext" -out "$TLS_DIR/harness.pem" 2>/dev/null
chmod 600 "$TLS_DIR"/*.key
{
  echo "export YOMI_TEST_HARNESS_ALLOWED_HOST='127.0.0.1'"
  echo "export YOMI_TEST_HARNESS_ALLOWED_IP='127.0.0.1'"
  echo "export YOMI_TEST_HARNESS_BLOCKED_HOST='127.0.0.2'"
  echo "export YOMI_TEST_HARNESS_DNS_PORT='$HARNESS_DNS_PORT'"
  echo "export YOMI_TEST_HARNESS_TLS_CERT='$TLS_DIR/harness.pem'"
  echo "export YOMI_TEST_HARNESS_TLS_KEY='$TLS_DIR/harness.key'"
  # CMSの設定の [egress] extra_ca_files に入れる（ドライバーの server-process.ts）
  echo "export YOMI_TEST_HARNESS_TLS_CA='$TLS_DIR/ca.pem'"
} >> "$OUT/test.env"

# 擬似IdP（tests/invariants/support/oidc-idp.ts）。テストが起動し、CMSはこの発行者をOIDCの接続先にする（例外の宛先 127.0.0.1）
OIDC_PORT="${YOMI_TEST_OIDC_PORT:-5380}"
{
  echo "export YOMI_TEST_OIDC_PORT='$OIDC_PORT'"
  echo "export YOMI_TEST_OIDC_ISSUER='http://127.0.0.1:$OIDC_PORT'"
  echo "export YOMI_TEST_OIDC_CLIENT_ID='yomi-inv-tests'"
  echo "export YOMI_TEST_OIDC_CLIENT_SECRET='$(random_pw)'"
  echo "export YOMI_TEST_OIDC_JIT_DOMAIN='jit.inv.test'"
} >> "$OUT/test.env"

if [ -n "$PG15_URL" ]; then
  HP15="$(host_port "$PG15_URL")"
  APP15="$(random_pw)"
  MIG15="$(random_pw)"
  create_roles "$PG15_URL" "$APP15" "$MIG15"
  {
    # 別のホストの PostgreSQL 15 でも、DB の版の検査に届くように、平文を明示する（TLS の起動時の検査で止めない）
    echo "export YOMI_TEST_PG15_URL='postgres://yomi_app:${APP15}@${HP15}/yomi?sslmode=disable'"
    echo "export YOMI_TEST_PG15_MIGRATION_URL='postgres://yomi_migrate:${MIG15}@${HP15}/yomi?sslmode=disable'"
  } >> "$OUT/test.env"
fi
chmod 600 "$OUT/test.env"
echo "Wrote $OUT/test.env"
