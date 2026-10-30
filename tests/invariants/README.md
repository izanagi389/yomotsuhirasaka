# 約束のテスト

`docs/invariants.md` の約束を、CMSの外から確かめるテストです。TypeScriptとVitestで書きます。

## 構成

| ファイル | 約束 |
|---|---|
| `auth.test.ts` | ログインの基本・CSRF・Cookie（AUTH-01〜03, 07, 08） |
| `auth-login.test.ts` | ログインの制限・管理者の多要素認証・パスワードの強さ・端末のCookie（AUTH-04, 05, 17, 21, 22, 24） |
| `auth-session.test.ts` | 再認証とセッションの失効（AUTH-06, 12, 13） |
| `auth-tokens.test.ts` | 個人トークンとキーの渡し方・デプロイの権限（AUTH-09, 11, 18, 20） |
| `auth-recovery.test.ts` | リセットと招待（AUTH-10, 14〜16） |
| `auth-oidc.test.ts` | OIDC/SSO（AUTH-19） |
| `emergency.test.ts` | 緊急停止と鍵の入れ替え（AUTH-23, KEY-03） |
| `tenancy.test.ts` | ワークスペースの分離（TEN-01〜06） |
| `api-keys.test.ts` | 配信APIのキー（API-01, 02, KEY-01, 04） |
| `graphql.test.ts` | 配信API（GQL-01〜08） |
| `publishing.test.ts` | 公開・版・抹消・権限・代替テキスト・訂正・URL・楽観ロック（PUB-01〜06, PERM-01, IMG-01, COR-01, URL-01, EDIT-01） |
| `body.test.ts` | アンカーIDと本文の検証（ANC-01, 02, VAL-01〜04） |
| `localization.test.ts` | 多言語（LOC-01, 03, 04） |
| `schema.test.ts` | スキーマとデプロイ（SCH-01〜07） |
| `webhooks.test.ts` | Webhookと外部への送信（WH-01〜07） |
| `media.test.ts` | メディア（MED-01〜06） |
| `audit.test.ts` | 監査ログと通知（AUD-01, 02, 04〜07） |
| `import.test.ts` | 取り込み（IMP-01〜03, LOC-05） |
| `headers.test.ts` | ヘッダーと公開ポート（HDR-01, 02, CFG-02） |
| `logs.test.ts` | ログ（LOGS-01） |
| `startup.test.ts` | 起動時の検査（KEY-02, CFG-01） |
| `db-roles.test.ts` | DBの権限（DB-01〜04, CFG-03） |
| `hooks.test.ts` | 組み込みJSとフック（HOOK-01〜05） |
| `corrections.test.ts` | 訂正履歴・削除済みアンカー・訂正フィード・鮮度（COR-02〜04, FRS-01） |
| `redirects.test.ts` | リダイレクト一覧（RED-01, 02） |
| `converter.test.ts` | 変換器とカスタムノード（NODE-01, CNV-01） |
| `permissions.test.ts` | 重い権限（PERM-02, 03） |
| `budget.test.ts` | 予算（BUD-01） |
| `answer.test.ts` | 質問応答（QA-01, 02, 04, 05, 10〜15） |
| `answer-cache.test.ts` | 回答キャッシュ（QA-06〜09） |
| `search.test.ts` | サイト内検索（SRC-01〜04） |
| `meta.test.ts` | 約束の一覧・テスト・`pending.txt` の整合。CMSなしで動く |

| 部品（`support/`） | 役割 |
|---|---|
| `inv.ts` | 約束のテストを登録する `inv()`。マイルストーンと未着手の一覧で、実行するかを決める |
| `driver.ts` | **ドライバー**。管理APIのURLや設定の書式などの実装の細部を、ここに閉じ込める |
| `admin-client.ts` ・`server-process.ts` | ドライバーの一部（管理APIへの接続、サーバーの設定の書き出しと起動） |
| `world.ts` | テストの舞台（ワークスペースA・B、利用者、公開記事、APIキー）を作る |
| `operations.ts` | 管理APIの操作の一覧と、必ずある操作の名前と向き先 |
| `template.ts` | テスト用のテンプレート（スキーマ）の定義 |
| `scripts.ts` | 組み込みJSで動かす関数（URL生成関数・フック・カスタムノードの変換関数）のソース。ホスト関数の呼び方などの実装の細部を閉じ込める |
| `html.ts` | 変換器が出力したHTMLの確認（スクリプトを実行できる要素や属性がないこと） |
| `body.ts` | 本文（TiptapのJSON）の組み立て |
| `assert.ts` ・`compare.ts` | 共通の確認（拒否・漏れ・区別できない応答） |
| `net.ts` | 受信サーバー（Webhookの受け手など） |
| `dns.ts` | `inv.test` のゾーンに答えるDNSサーバー（DNSリバインディングの確認用） |
| `proxy.ts` | 中継の依頼を記録する出口のプロキシ |
| `oidc-idp.ts` | 擬似IdP（OIDC） |
| `hibp.ts` | 擬似の漏えいパスワードの照会先 |
| `mock-llm.ts` | 擬似LLM（`tools/mock-llm`）の起動と操作（障害の設定・回答の組み立て方・受け取ったリクエスト。APIキーごと） |
| `images.ts` | 位置情報入りの画像、解凍爆弾、許可しない形式のファイルなど |
| `mail.ts` | テスト環境のメールの受け口から、メールとトークンを取り出す |
| `totp.ts` | 多要素認証のコード（RFC 6238） |
| `db.ts` | DBへの直接の接続。使えるのは、DBの権限の約束と、「保存しない」ことと「時刻」に関わる約束（QA-08, 09, LOG-01, 02）のテストだけ。後者はドライバーの `driver.storage` を通す |

## 約束のテストの書き方

```ts
inv("inv_ten_06", "サーバー管理者以外は、ワークスペースを作成できない", async () => {
  // ...
});
```

- 第1引数はテスト名、第2引数は `docs/invariants.md` の約束の文をそのまま書く。一字でも違うとテストの読み込みで止まる。
- マイルストーンは `docs/invariants.md` から読む。`current-milestone` より先のものは自動で保留になる。
- 拒否の確認には、必ず対照（成功するはずの操作）を付ける。「何でも拒否する」実装で通ってしまわないようにするため。
- 漏れてはいけない値は、一意な目印（`marker()`）を記事などに入れ、応答に含まれないことを確かめる。
- 時間に関わる約束（有効期限、保存期間、再認証の有効時間など）は、その値を短くしたサーバーを `driver.startServer` で別に起動して確かめる。
- インスタンス全体に効く操作（緊急停止、鍵の入れ替え、出口のプロキシ）は、`driver.createIsolatedInstance` の新しいインスタンスで確かめる。
- 「保存しない」ことと「時刻」に関わる約束（QA-08, 09, LOG-01, 02）だけは、`driver.storage` でDBに直接つないでよい（CLAUDE.md）。保存しないことは、DBの全テキストの列に目印がないことで確かめ、対照として、保存されているはずの目印が見つかることも確かめる。時刻は、記録された時刻を過去にずらすことだけに使う。

### 応答の状態コードの前提

| 場面 | 状態コード |
|---|---|
| 認証・認可の失敗（ログインしていない、権限がない、他のワークスペース） | 401・403・404 のどれか（`expectRejected`） |
| 検証の失敗、未登録のクエリ、移行待ちの編集など | 4xx（`expectRefused`） |
| 回数制限 | 429 |
| 下書きの保存のリビジョン番号の食い違い | 409 |

## ドライバー

テストは「何を確かめるか」だけを書き、「どのURLを呼ぶか」はドライバーが持ちます。管理APIの形が決まるまで、ドライバーの関数は `NotImplementedError` を投げます。

- ドライバーの中身は、実装を担当するAIが、APIを作るのと同時に書く。
- ドライバーには合否の判定を書かない。失敗を成功に見せかけない（セットアップの関数は、CMSが失敗を返したら例外を投げる）。
- `adminOperations` には、ログインが必要な管理APIのすべての操作を載せる。`support/operations.ts` の必須の操作が欠けているか、向き先が違うとテストは失敗する。
- ドライバーの変更も持ち主のレビューを受ける（`tests/invariants/` はCODEOWNERSの対象）。レビューでは、**ドライバーが手を抜いていないか**（操作を省いていないか、エラーを握りつぶしていないか）を見る。

## 未着手の一覧（`pending.txt`）

- 約束を実装してテストが通るようになったら、実装を担当するAIがその行を削除する。
- 行を追加する（テストの実行をやめる）のは、持ち主の承認があるときだけ。
- 現在のマイルストーンの行がすべて消えたら、そのマイルストーンは完了。
- `meta.test.ts` が、現在のマイルストーン以前の約束に「テストがあるか、`pending.txt` に載っているか」を確かめる。

## テスト環境

テスト用のDocker Composeで次を用意し、環境変数で渡す（Composeファイルは実装と一緒に作る）。

### CMSとDB

| 環境変数 | 内容 |
|---|---|
| `CMS_TEST_URL` | 本番の設定で起動したCMS（HTTPS）。`support/template.ts` の `BASE_TEMPLATE` をデプロイ済み |
| `CMS_TEST_SERVER_ADMIN_EMAIL` ・`_PASSWORD` ・`_TOTP_SECRET` | 初期化で作ったサーバー管理者（多要素認証を設定済み） |
| `CMS_TEST_DB_APP_URL` | CMSが普段の処理に使うのと同じDBの接続先 |
| `CMS_TEST_DB_SUPERUSER_URL` | DBのスーパーユーザー。テーブルの一覧を調べることと、スーパーユーザーで起動しないことの確認にだけ使う |
| `CMS_TEST_PG15_URL` ・`CMS_TEST_PG15_MIGRATION_URL` | PostgreSQL 15 の普段用と移行用の接続先（それぞれ、スーパーユーザーでないロールと、CREATEROLE を持つロール）。16より古いDBで起動しないことの確認用 |
| `CMS_TEST_FS_ROOT` | CMSのメディアの保存先を含む上位のディレクトリ（テストの実行環境からも見えるようにマウントする） |

### テストの実行環境（ハーネス）

テストの実行環境は、受信サーバー・DNS・擬似IdPなどを自分で立てる。CMSからは次の名前とアドレスで届くようにする。

| 環境変数 | 内容 |
|---|---|
| `CMS_TEST_HARNESS_ALLOWED_HOST` ・`_ALLOWED_IP` | CMSの外部への送信の例外として許可しておく、テストの実行環境のホスト名とIPアドレス（例外にはIPアドレスで登録し、HTTPも許す） |
| `CMS_TEST_HARNESS_BLOCKED_HOST` | テストの実行環境のもう1つのIPアドレス。例外に入れない（内部向けのアドレスとして拒否されるべき宛先） |
| `CMS_TEST_HARNESS_DNS_PORT` | テストの実行環境のDNSサーバーのUDPのポート。CMSは `inv.test` のゾーンをこのDNSサーバーに問い合わせるようにする（他の名前は通常どおり） |
| `CMS_TEST_HARNESS_TLS_CERT` ・`_KEY` | `*.inv.test` に有効な証明書と鍵。CMSはこの証明書のCAを信頼する |
| `CMS_TEST_HARNESS_TLS_CA` | 上の証明書のCA。ドライバーが、CMSの設定の `[egress] extra_ca_files` に入れる |
| `CMS_TEST_OIDC_PORT` ・`_ISSUER` ・`_CLIENT_ID` ・`_CLIENT_SECRET` | 擬似IdP。CMSにはこの発行者をOIDCの接続先として設定する |
| `CMS_TEST_OIDC_JIT_DOMAIN` | JIT（自動作成）を有効にするドメイン |

- 回数制限が他のテストに波及しないよう、テストの実行環境を「信頼するプロキシ」として設定し、テストごとに `X-Forwarded-For` で別の接続元を名乗る（Q106）。
- メールはSMTPの受け手（Mailpitなど）で受け、ドライバーの `mail` で読む。
- `headers.test.ts` の公開ポートの確認には、`docker` コマンド（`docker compose config`）を使う。
- 自己署名の証明書を使う場合は、`NODE_EXTRA_CA_CERTS` でCAの証明書を渡す。
- `scripts/test-env/setup.sh` が、ハーネスの環境変数（例外として許す宛先 `127.0.0.1`、許さない内部のアドレス `127.0.0.2`、DNSのポート `5354`、TLSのCAと証明書）も書き出す。
- PostgreSQL 16 には pgvector が入っていること（CI は `pgvector/pgvector:pg16`）。`scripts/test-env/setup.sh` と `driver.createIsolatedInstance` が、スーパーユーザーで拡張 `vector` を作る。日本語のキーワード検索の拡張（pg_bigm）は入れない（INV-SRC-05）。
- 擬似LLMは、ハーネスがビルド済みの `tools/mock-llm/dist/main.js`（`npm run build -w @new-cms/mock-llm`）を別のプロセスとして起動し、`CMS_TEST_MOCK_LLM_URL`（OpenAI互換の `…/v1`）に入れる。別の場所なら `CMS_TEST_MOCK_LLM_BIN` で指定する。外で用意したCMS（`CMS_TEST_URL`）を使うときは、`CMS_TEST_MOCK_LLM_URL` も渡す。
- 取り込みのテストは、ビルド済みの取り込みのCLI（`cli/dist/index.js`。`cd cli && npm ci && npm run build`）を実行する。別の場所なら `CMS_TEST_CLI_BIN` で指定する。
- `inv.test` の名前解決は、`scripts/test-env/harness-dns.sh`（root）で、`127.0.0.1:53` の dnsmasq からテストのDNSサーバーへ回す。CMSはOSの名前解決をそのまま使う。

## 実行

```sh
npm install
npm run typecheck
npm test
```
