#!/usr/bin/env bash
# 約束のテストのための名前解決の設定（CIとローカルのLinuxで共通。root で実行する）。
#
# 使い方: sudo scripts/test-env/harness-dns.sh [テストのDNSサーバーのポート（既定 5354）]
#
# CMSは、OSの名前解決（/etc/resolv.conf）をそのまま使う。テストでは `inv.test` のゾーンだけを、
# テストの実行環境で動かすDNSサーバー（tests/invariants/support/dns.ts）に回したいので、
# 127.0.0.1:53 で dnsmasq を動かし、`inv.test` はテストのDNSサーバーへ、それ以外は元の問い合わせ先へ転送する。
# - キャッシュは持たない（DNSリバインディングの確認で、問い合わせのたびに答えを変えるため）
# - 元の /etc/resolv.conf は /etc/resolv.conf.cms-test.bak に残す
set -euo pipefail

PORT="${1:-5354}"
CONF=/etc/cms-test-dnsmasq.conf
command -v dnsmasq >/dev/null || { echo "dnsmasq is not installed (apt-get install dnsmasq-base)" >&2; exit 1; }

# 元の問い合わせ先（自分自身は除く）
RESOLV="$(readlink -f /etc/resolv.conf)"
[ -f /etc/resolv.conf.cms-test.bak ] || cp "$RESOLV" /etc/resolv.conf.cms-test.bak
UPSTREAMS="$(awk '$1 == "nameserver" && $2 != "127.0.0.1" { print $2 }' /etc/resolv.conf.cms-test.bak)"
[ -n "$UPSTREAMS" ] || { echo "no upstream nameserver in /etc/resolv.conf" >&2; exit 1; }

{
  echo "listen-address=127.0.0.1"
  echo "port=53"
  echo "bind-interfaces"
  echo "no-resolv"
  echo "cache-size=0"
  echo "server=/inv.test/127.0.0.1#${PORT}"
  for ns in $UPSTREAMS; do echo "server=${ns}"; done
} > "$CONF"

pkill -f "dnsmasq --conf-file=${CONF}" 2>/dev/null || true
dnsmasq --conf-file="$CONF" --pid-file=/run/cms-test-dnsmasq.pid

# /etc/resolv.conf がリンク（systemd-resolved など）のときは、リンクを置き換える
rm -f /etc/resolv.conf
printf 'nameserver 127.0.0.1\noptions timeout:2 attempts:2\n' > /etc/resolv.conf
echo "dnsmasq: inv.test -> 127.0.0.1#${PORT}, others -> $(echo $UPSTREAMS | tr '\n' ' ')"
