# 開発環境

## 手元での起動（Q71）

- 最初に初期化のコマンド（サーバーのバイナリのサブコマンド。例: `cms-server init`、Q180）で秘密の値と暗号化の鍵を生成する。鍵はDBとは別のファイルに書き出される（Q172、`13-security.md`）。
- 公式のDocker Composeファイルを提供し、次を一式起動できるようにする。
  - CMSサーバー
  - 拡張（pgvector、pg_bigm または PGroonga）入りのPostgreSQL
  - 擬似LLM

```yaml
services:
  cms:
    image: (CMS名)/server
    environment:
      LLM_BASE_URL: http://mock-llm:8080/v1   # 本番ではここを実際の接続先に変えるだけ
  db:
    image: (CMS名)/postgres
  mock-llm:
    image: (CMS名)/mock-llm
```

## 擬似LLM（Q71）

- OpenAI互換のAPI（埋め込みと、ストリーミング対応のチャット）だけを持つ小さなサーバーを、別のコンテナとして同梱する。
- CMSからは普通の接続先として見える。本番のコードに擬似用の分岐を入れない。
- 擬似LLMはHTTPで内部向けの宛先なので、開発用のDocker Composeには、外部への送信の例外（内部宛先とHTTPの許可）の設定を最初から入れておく（Q139、`13-security.md`）。
- 埋め込みは、入力の文字列から決まった値を返す（同じ入力には常に同じ値）。
- 回答は、プロンプトに含まれる断片をもとに、CMSの形式（判定の先行宣言、`[1]` 形式の引用番号）に沿った回答を決まった手順で組み立てて返す。
- 設定で障害を起こせるようにする: タイムアウト、引用番号のない回答、実在しない出典番号の宣言、429、途中で切れるストリームなど。
- 受け取ったリクエストを記録し、テストから確認できるようにする（`devNote` がプロンプトに含まれないことの確認などに使う）。
- CMSのプロンプトの形式と密接に結びつくため、CMSと同じリポジトリでバージョン管理する。

## テスト

- 約束のテストの規則は `CLAUDE.md` の「約束のテスト」を参照。
- 約束のテストはTypeScriptとVitestで書き、`tests/invariants/` に置く。サーバーと別の言語にするのは、持ち主がRustを細かく読まなくてもテストをレビューできるようにするため。実装の細部はドライバー（`tests/invariants/support/driver.ts`）に閉じ込める（Q182）。
- 実装前のテストは `tests/invariants/pending.txt` に載せて実行を保留し、実装したら外す。一覧が空になればそのマイルストーンは完了（Q183）。
- テスト環境（本番の設定で起動したCMS、DBの接続先、PostgreSQL 15など）の詳細は `tests/invariants/README.md`。
- テスト用のDB（PostgreSQL 16と15のロールとDB）、設定ファイル、暗号化の鍵、テストに渡す環境変数は `scripts/test-env/setup.sh` で作る。CIもこのスクリプトを使う。
- LLMが関わるテストには、本物のLLMではなく擬似LLMを使う。
