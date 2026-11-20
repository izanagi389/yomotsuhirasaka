#!/usr/bin/env bash
# Claude がファイルを編集した直後に、そのファイルだけを整形する（.claude/settings.json の PostToolUse）。
# - server/ の .rs: チェック用のコンテナ（deploy/docker-compose.tools.yml）の rustfmt。コンテナが止まっていれば何もしない
# - admin/ の .ts・.vue: eslint --fix
# 整形できなくても作業は止めない（常に 0 で終わる）。直せない lint の違反は、作業の流れのチェックで見つける。
set -uo pipefail

repo_root="${CLAUDE_PROJECT_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
file_path="$(jq -r '.tool_input.file_path // empty' 2>/dev/null)"
[ -n "$file_path" ] && [ -f "$file_path" ] || exit 0

case "$file_path" in
  "$repo_root"/*) relative="${file_path#"$repo_root"/}" ;;
  *) exit 0 ;;
esac

case "$relative" in
  server/*.rs)
    compose=(docker compose -f "$repo_root/deploy/docker-compose.tools.yml")
    if [ -n "$("${compose[@]}" ps --status running -q rust 2>/dev/null)" ]; then
      "${compose[@]}" exec -T rust rustfmt --edition 2024 "/src/$relative" >/dev/null 2>&1 || true
    fi
    ;;
  admin/*.ts | admin/*.vue)
    (cd "$repo_root/admin" && npx --no-install eslint --fix "$file_path" >/dev/null 2>&1) || true
    ;;
esac
exit 0
