#!/usr/bin/env bash
# DB の結合テストも含めて、cargo test をチェック用のコンテナで実行する。
# テスト専用の PostgreSQL（deploy/docker-compose.tools.yml のプロファイル db）を起動し、接続先を渡す。
# テストは毎回、使い捨ての DB とロールを作って消すので、CMS の DB や記事のデータには触れない。
#
#   scripts/cargo-db-test.sh                    # すべてのテスト
#   scripts/cargo-db-test.sh --test keyword_search
#
# 結合テストはファイルごとに実行ファイルができ、リンクのメモリとディスクを多く使う。
# そのため CI と同じくデバッグ情報を行番号だけにし、並列のビルドは CARGO_BUILD_JOBS（既定は 1）に絞る。
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
compose=(docker compose -f "$repo_root/deploy/docker-compose.tools.yml")

"${compose[@]}" --profile db up -d --wait rust pg16 pg-bigm >&2

exec "$repo_root/scripts/cargo.sh" --exec env \
  YOMI_TEST_PG_SUPERUSER_URL=postgres://postgres:local-test-only-pw-16@127.0.0.1:5432/postgres \
  YOMI_TEST_PG_BIGM_URL=postgres://postgres:local-test-only-pw-bigm@127.0.0.1:5434/postgres \
  CARGO_INCREMENTAL=0 \
  CARGO_PROFILE_DEV_DEBUG=line-tables-only \
  CARGO_PROFILE_TEST_DEBUG=line-tables-only \
  CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}" \
  cargo test --locked "$@"
