#!/usr/bin/env bash
# Rust のコマンドを、チェック用のコンテナ（deploy/docker-compose.tools.yml）の中で実行する。
# コンテナが止まっていれば起動してから実行する。
#
#   scripts/cargo.sh fmt --check
#   scripts/cargo.sh clippy --locked --all-targets -- -D warnings
#   scripts/cargo.sh test --locked
#   scripts/cargo.sh --exec rustfmt --edition 2024 server/src/lib.rs   # cargo 以外のコマンド
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
compose=(docker compose -f "$repo_root/deploy/docker-compose.tools.yml")

if [ -z "$("${compose[@]}" ps --status running -q rust 2>/dev/null)" ]; then
  "${compose[@]}" up -d --wait rust >&2
fi

tty_flag=()
if [ ! -t 0 ] || [ ! -t 1 ]; then
  tty_flag=(-T)
fi

if [ "${1:-}" = "--exec" ]; then
  shift
  exec "${compose[@]}" exec "${tty_flag[@]}" rust "$@"
fi
exec "${compose[@]}" exec "${tty_flag[@]}" rust cargo "$@"
