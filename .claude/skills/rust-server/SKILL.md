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

`scripts/cargo.sh test` では、DB を使う結合テスト（`server/tests/`）が飛ばされる（`skipped:` と出る）。DB のテストも通すときは次を使う。テスト専用の PostgreSQL（CI と同じイメージ）を起動して接続先を渡す。テストは使い捨ての DB を作って消すので、CMS の DB には触れない。

```bash
scripts/cargo-db-test.sh                     # すべてのテスト（DB のテストを含む）
scripts/cargo-db-test.sh --test admin_auth   # 一部だけ
```

- 並列のビルドは `CARGO_BUILD_JOBS`（既定は 1）で決まる。メモリが少ない環境では、並列を増やすとリンクでメモリが足りなくなる。`-j 2` 程度から試す。
- すべてのテストには10分以上かかる。バックグラウンドで実行し、出力はファイルに残す（`head` などで途中を切ると、合計の件数を数え直せない）。件数は、全行の `test result:` の `passed` と `failed` を足して確かめる。

## SQL と移行

- SQL の識別子（スキーマ名・テーブル名など）は `crate::db::Ident` と `crate::sql!` だけで埋め込む。値は必ずパラメーター（`$n`）で渡す。定数と `$n` だけで組み立てた文は `crate::db::trusted` に渡す（13-security.md「SQL」、CLAUDE.md の禁止事項）。
- ワークスペースのスキーマの移行は `server/migrations/workspace/` に連番で足し、`server/src/workspace_db.rs` の `BASE_MIGRATIONS` に登録する。共通のスキーマの移行は `server/migrations/` に足し、`server/src/migrate.rs` の `MIGRATIONS` に登録する。適用済みの移行のファイルは書き換えない（適用済みの環境では流し直されないため）。直すときは新しい移行を足す。
- ワークスペースの移行は、普段の処理の DB の権限ではなく、移行用の接続で実行される。移行で作ったテーブルには、ワークスペースのロールの読み書きの権限が自動で付く（`workspace_db.rs` の `ALTER DEFAULT PRIVILEGES`）。DDL の権限は付かない（INV-DB-02）。

## エラー処理

- エラーは `anyhow` で扱い、`.context()` / `.with_context()` で「何をしていたときの失敗か」を付ける。`thiserror` などの新しい依存は追加しない。
- 新しく書く本番のコード（`#[cfg(test)]` と `server/tests/` 以外）では、`unwrap()` と `expect()` を使わない。`?` で返すか、失敗し得ない理由がある場合は、その理由を書いたうえで既存のコードの扱いに合わせる。
- エラーのメッセージに、パスワード、トークン、APIキー、Cookie、記事の本文、質問文を含めない（CLAUDE.md の禁止事項）。

## unsafe

- 原則として使わない。どうしても必要なときは、利用者の承認を受け、ブロックの直前に `// SAFETY:` コメントで、守られている前提を書く。

## clippy

- `clippy::pedantic` を有効にしている（ルートの `Cargo.toml` の `[workspace.lints.clippy]`）。CI は `-D warnings` なので、pedantic の警告も CI で止まる。確かめるのは上の `scripts/cargo.sh clippy --locked --all-targets -- -D warnings` だけでよい。
- 次の7つは対象外（直すとかえって読みにくくなる、または lint の修正では済まない大きな分割になるため）。対象を変えるときは `Cargo.toml` を直し、利用者の承認を受ける。
  - 説明文と属性: `missing_errors_doc`、`missing_panics_doc`、`must_use_candidate`
  - 読みやすさを下げるもの: `too_many_lines`、`assert_is_empty`、`format_collect`、`similar_names`
- 対象外でも、関数は短く保つ（`code-quality` の「小さく保つ」）。`too_many_lines` を外したのは、既存の長い関数の分割を lint の修正と混ぜないため。
- 説明文の固有名詞（`MaxMind`、`IdP` など）を `doc_markdown` がコード名と誤認するときは、バッククォートで囲まずに `clippy.toml` の `doc-valid-idents` に足す。日本語に続けて書くと1語と見なされるので、前後に空白を入れる。
- `schema::manifest` の型の説明文は、テンプレートの JSON Schema（`cli/schema/template.schema.json`）に出る。lint のために文を変えず、理由を書いた `#[allow]` にする。

- 数値の型変換（`as`）は、切り捨てやあふれが起きないことが明らかな場合を除き、`try_from` などで失敗を扱う。
- lint を `#[allow(...)]` で黙らせるときは、理由をコメントに書く。

## 性能

- 計測せずに最適化しない（`code-quality` の優先順位）。
- ベンチマークのための依存（`criterion` など）は、必要になった時点で CLAUDE.md の手順で承認を求める。
- DB への問い合わせをループの中で繰り返さない（N+1 にしない）。
- 一覧の問い合わせでは、`LIMIT` の前に大きな列（本文の `jsonb`、`::text` への変換など）を読まない。PostgreSQL は並べ替えの前に、該当する全行の SELECT の式を計算する。軽い列でページの行を選んでから、その行だけを読む（`delivery::data::list`）。
- 絞り込みのたびに本文を含む `jsonb` 全体を読まない。必要な短い値だけを、生成列などの小さな列に分ける（`versions.list_values`）。

### 計測のしかた

- テスト用の PostgreSQL（`scripts/cargo-db-test.sh` が起動する pg16。CMS の DB とは別）に、計測用の DB を作る。データは `generate_series` で入れ、問い合わせは実装が組み立てるのと同じ SQL を `EXPLAIN (ANALYZE, BUFFERS)` で流す。何度か流して、ばらつきも見る。
- 本文は数KB以上にして、TOAST される大きさにする（小さな本文では、本文を読む費用が見えない）。
- この PostgreSQL のデータは tmpfs（メモリの上。使える大きさは Docker に割り当てたメモリで決まり、数GBしかないことがある）に置かれる。大量のデータを入れるときは、行を UPDATE で書き直さずに1回の INSERT で作る（書き直すと一時的に倍の容量を使い、空きがなくなる）。
- 計測が終わったら、計測用の DB を消す（残すと、DB のテストが容量不足で失敗し得る）。
- 結果（件数、条件、変更前後の時間）は PR の説明に書き、コードのコメントには理由だけを短く書く。
