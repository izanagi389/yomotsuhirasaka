---
name: rust-server
description: server/（Rust のCMSサーバー）のコードを書く・直すときの決まり。エラー処理、unwrap・unsafe の扱い、clippy の基準、手元でのチェックの実行方法。server/ の .rs を変更する作業で読む。
---

# Rust（server/）の決まり

全体の基準は `code-quality` スキル、禁止事項は CLAUDE.md に従う。

## 手元での実行

このマシンには Rust を入れていない。コマンドはチェック用のコンテナ（`deploy/docker-compose.tools.yml`）で動かす。

```bash
scripts/cargo.sh fmt --check
scripts/cargo.sh clippy --locked --all-targets -- -D warnings
scripts/cargo.sh test --locked
```

ツールチェーンは `deploy/server.Dockerfile` の `toolchain` の段階と同じ版。

## エラー処理

- エラーは `anyhow` で扱い、`.context()` / `.with_context()` で「何をしていたときの失敗か」を付ける。`thiserror` などの新しい依存は追加しない。
- 新しく書く本番のコード（`#[cfg(test)]` と `server/tests/` 以外）では、`unwrap()` と `expect()` を使わない。`?` で返すか、失敗し得ない理由がある場合は、その理由を書いたうえで既存のコードの扱いに合わせる。
- エラーのメッセージに、パスワード、トークン、APIキー、Cookie、記事の本文、質問文を含めない（CLAUDE.md の禁止事項）。

## unsafe

- 原則として使わない。どうしても必要なときは、利用者の承認を受け、ブロックの直前に `// SAFETY:` コメントで、守られている前提を書く。

## clippy

- CI は `-D warnings`（標準の lint）。
- 新しく書くコードは、`clippy::pedantic` でも警告が出ないように書く。ただし `missing_errors_doc`、`missing_panics_doc`、`must_use_candidate` の3つは対象外。確かめるときは次を実行し、変更したファイルの警告がないことを見る。

```bash
scripts/cargo.sh clippy --locked --all-targets --message-format=short -- -W clippy::pedantic -A clippy::missing_errors_doc -A clippy::missing_panics_doc -A clippy::must_use_candidate
```

- 数値の型変換（`as`）は、切り捨てやあふれが起きないことが明らかな場合を除き、`try_from` などで失敗を扱う。
- lint を `#[allow(...)]` で黙らせるときは、理由をコメントに書く。

## 性能

- 計測せずに最適化しない（`code-quality` の優先順位）。
- ベンチマークのための依存（`criterion` など）は、必要になった時点で CLAUDE.md の手順で承認を求める。
- DB への問い合わせをループの中で繰り返さない（N+1 にしない）。
