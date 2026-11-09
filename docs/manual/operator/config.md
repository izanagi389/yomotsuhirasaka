# 設定ファイル

`yomi-server init` が作る TOML のファイルです（公式の Docker Compose では `./data/yomi/config.toml`）。知らない項目があると起動しません。変えたらサーバーを再起動してください。

相対パスは、設定ファイルのあるディレクトリを基準にします。

**このファイルには DB のパスワードなどの秘密の値が入ります。** ほかの人が読めないようにしてください。

## 全体

| 項目 | 既定 | 内容 |
|---|---|---|
| `production` | 必須 | 本番の設定か。本番では起動時の検査に1つでも当てはまると起動しません |
| `dev_mode` | `false` | 開発モード（任意のクエリを受け付ける）。本番では有効にできません |
| `public_url` | 必須 | 公開する URL。本番では `https://`。リセットや招待のリンクはこれだけから作ります |
| `listen` | 必須 | 待ち受けるアドレス |
| `log_level` | `info` | ログの詳しさ（`error`、`warn`、`info`、`debug`、`trace`） |
| `encryption_key_file` | | 暗号化の鍵のファイル |
| `trusted_proxies` | `[]` | `X-Forwarded-For` を信頼するプロキシの IP アドレス。ここにないところからの値は無視します |
| `default_locale` | `en` | インスタンスの既定の言語（`ja`、`en`）。言語を設定していない利用者へのメールに使います |

### 起動時の検査

本番の設定では、次のどれかに当てはまると、理由を表示して起動を止めます。

- 秘密の値（DB のパスワード）が空、16文字より短い、既知の初期値、同じ文字の繰り返し
- DB の接続に使うユーザーがスーパーユーザー
- `dev_mode` が有効
- `public_url` が https でない
- 暗号化されない SMTP を、ループバック以外の宛先に指定している

次は本番でなくても起動を止めます: 暗号化の鍵がない、PostgreSQL が16より古い、接続先の URL にパスワードが入っている。

## データベース（`[database]`、`[migration_database]`）

| 項目 | 内容 |
|---|---|
| `url` | 接続先の URL。**パスワードは含めません** |
| `password` | パスワード |

`[database]` は普段の処理用です（DDL はできません）。`[migration_database]` は移行とワークスペースのロールの作成用で、`CREATEROLE` と DDL の権限を持ちます。どちらもスーパーユーザーにはできません。

## セッション（`[session]`）

| 項目 | 既定 | 内容 |
|---|---|---|
| `absolute_lifetime_seconds` | 43200（12時間） | ログインからの最大時間 |
| `idle_timeout_seconds` | 7200（2時間） | 無操作の期限 |
| `reauth_window_seconds` | 600（10分） | 再認証が有効な時間 |
| `one_time_token_lifetime_seconds` | 1800（30分） | リセットと招待のリンクの有効期限 |

## 回数の制限（`[limits]`）

| 項目 | 既定 | 意味 |
|---|---|---|
| `invalid_api_keys_per_ip_per_minute` | 600 | 接続元ごとの、無効な配信 API のキーの上限 |
| `request_timeout_seconds` | 120 | リクエストの処理時間（本文の読み込みを含みます。デプロイと取り込みは除きます）。超えると `request_timeout`（503） |
| `max_concurrent_requests` | 2048 | 同時に処理するリクエストの数。超えると `server_busy`（503） |


| 項目 | 既定 | 内容 |
|---|---|---|
| `login_attempts_per_ip_per_minute` | 30 | IP アドレス（IPv6 は /64）ごとの、1分あたりのログインの試行 |

## 認証（`[auth]`）

| 項目 | 既定 | 内容 |
|---|---|---|
| `mfa_required_for_everyone` | `true` | 全員に多要素認証を必須にする |

### 漏えいしたパスワードの照会（`[auth.breached_passwords]`）

既定では照会しません。**有効にすることを勧めます。** パスワードの SHA-1 の先頭5文字だけを送ります。届かないときは、登録を止めずに長さとパターンの検査だけで通します。

```toml
[auth.breached_passwords]
base_url = "https://api.pwnedpasswords.com"
timeout_seconds = 3
```

### SSO（`[auth.oidc]`）

```toml
[auth.oidc]
issuer = "https://idp.example.com"
client_id = "…"
client_secret = "…"
display_name = "社内SSO"          # ログインの画面のボタンの名前。既定は SSO
jit_domains = []                  # 空なら招待制（既定）
```

- IdP のリダイレクト先には `<public_url>/api/admin/session/oidc/callback` を登録します。
- `jit_domains` を書くと、ID トークンの `email_verified` が真で、ドメインが一致する人のアカウントを初回のログインで作ります。
- メールアドレスが同じというだけで、既存のアカウントにはひも付けません。招待の承諾か、ログイン中の本人の操作でひも付けます。
- サーバー管理者とワークスペースの管理者は、SSO でも CMS 側のパスキーの登録が要ります。
- **社内の IdP につなぐときは、そのホストを `[egress] internal_destination_exceptions` に入れてください。** OIDC の設定の取得も外部への送信の共通の仕組みを通るので、内部のアドレスには届きません。

## メール（`[smtp]`）

```toml
[smtp]
host = "smtp.example.com"
port = 587                 # 省略できます（starttls は 587、tls は 465）
security = "starttls"      # starttls（既定）、tls、none
username = "…"
password = "…"
from = "CMS <yomi@example.com>"
```

`none` は同じホストの中継（ループバックの宛先）にだけ使えます。

SMTP を設定しないと、次のようになります。

- 本人への通知（新しい端末からのログイン、パスワードの変更など）は送りません。
- リセットと招待のリンクは、発行した人の画面に一度だけ出ます。
- 招待できるのはサーバー管理者だけになります。
- 「パスワードを忘れた場合」のメールは届きません。

## ログインの履歴（`[login_history]`）

| 項目 | 既定 | 内容 |
|---|---|---|
| `retention_seconds` | 7776000（90日） | 保存期間 |
| `sweep_interval_seconds` | 3600 | 削除の間隔 |
| `geoip_database` | なし | 国を調べるデータ（MaxMind DB 形式）。指定したときだけ、履歴に国を残し、国の変化を本人に知らせます。データは同梱しません |

## メディア（`[media]`）

| 項目 | 既定 | 内容 |
|---|---|---|
| `root` | 必須 | 保存先のディレクトリ（ローカルのファイルシステムだけ） |
| `public_base_url` | `public_url` の `/media` | 配信に使う URL の先頭 |
| `max_pixels` | 50000000 | 1枚の画像の画素数の上限 |
| `max_file_bytes` | 20971520（20MB） | 1つのファイルの大きさの上限 |
| `default_workspace_quota_bytes` | 5368709120（5GB） | ワークスペースの容量の上限 |
| `processing_timeout_seconds` | 20 | 画像の処理の時間の上限 |

**メディアは管理画面とは別のドメイン（例: `https://media.example.com`）から配信することを勧めます。** 設定がなくてもスクリプトの実行を禁じるヘッダーは付きますが、別のドメインなら、より安全です。

下書きや非公開の記事で使っているメディアも、URL を知っていれば取得できます。名前はランダムで推測できません。メディアを削除するとファイルも消えます。

## 配信API（`[delivery]`）

クエリの実行には次の上限があります。超えると、`query_timeout`（503）、`query_too_large`（422）、`delivery_busy`（503）を返します。

| 項目 | 既定 | 意味 |
|---|---|---|
| `max_query_seconds` | 15 | 1回のクエリの実行時間 |
| `max_resolved_entries` | 10000 | 1回のクエリで解決する記事の数 |
| `max_concurrent_queries_per_workspace` | 6 | ワークスペースごとの同時に実行するクエリの数 |
| `max_concurrent_queries` | 12 | インスタンス全体の同時に実行するクエリの数（DB の接続は全体で16本です） |

| 項目 | 既定 | 内容 |
|---|---|---|
| `max_depth` | 15 | クエリの深さの上限 |
| `max_complexity` | 500 | クエリの複雑さの上限 |
| `build_key_requests_per_minute` | 600 | 用途がビルド用のキーの回数 |
| `server_key_requests_per_minute` | 6000 | 用途がサーバー用のキーの回数 |
| `workspace_requests_per_minute` | 30000 | ワークスペースごとの回数 |
| `max_response_bytes` | 5242880（5MB） | 応答の大きさの上限 |
| `shared_cache` | `false` | 共有キャッシュを許すか |
| `shared_cache_seconds` | 60 | 共有キャッシュに載せる時間 |
| `query_check_unused_days` | 90 | デプロイのときに検証するクエリの範囲（これより長く使われていないものは検証しません） |

### 共有キャッシュ

`shared_cache = true` にすると、`read` の範囲だけのキーで実行した登録済みのクエリの応答に `s-maxage` が付きます。有効にする前に、次を理解してください。

- **CDN に載った応答は、API キーの照合なしに、URL を知っている人なら誰でも取得できます。**
- 記事を非公開にしたときの即時性は、CMS の応答についての約束です。CDN に残った応答は CMS の外にあります。記事の状態の Webhook で CDN のキャッシュを消してください。

## 回答API（`[answer]`）

| 項目 | 既定 | 内容 |
|---|---|---|
| `key_requests_per_minute` | 60 | キーごとの1分あたりの回答の回数 |
| `visitor_requests_per_minute` | 10 | 訪問者ごとの1分あたりの回数 |
| `visitor_requests_per_day` | 100 | 訪問者ごとの1日あたりの回数 |

## 外部への送信（`[egress]`）

Webhook、監査ログの書き出し、フックからの通信、LLM、OIDC のすべてがこの仕組みを通ります。送信先は原則 https で、内部向けのアドレス（プライベート、ループバック、リンクローカル、メタデータサーバーなど）には送りません。リダイレクトはたどりません。`HTTP_PROXY` などの環境変数は読みません。

| 項目 | 既定 | 内容 |
|---|---|---|
| `internal_destination_exceptions` | `[]` | 例外として許す内部の宛先（ホスト名か IP アドレス）。http での送信も許します |
| `extra_ca_files` | `[]` | 追加で信頼する CA の証明書（PEM）のファイル |
| `connect_timeout_seconds` | 10 | 接続の上限の時間 |
| `request_timeout_seconds` | 30 | 1回の送信全体の上限の時間 |

例外には必要な宛先だけを書いてください。Webhook やフックもその宛先に送れるようになります。

### 出口のプロキシ（`[egress.proxy]`）

```toml
[egress.proxy]
url = "http://proxy.example.com:3128"
connect_by = "ip"       # ip（既定）か hostname
```

- `ip` は、CMS が検査した IP アドレスを中継の宛先にします。検査したアドレスへの固定を保てます。
- `hostname` は、ホスト名でしか中継を許さないプロキシ向けです。**送信前の検査は行いますが、検査の後に名前解決の結果が変わる攻撃を防げるかはプロキシ次第になります。**
- プロキシの認証には対応していません。URL に認証情報を書かないでください。

## LLM（`[llm]`）

[LLMの設定](llm.md) を見てください。

## 検索（`[search]`）

| 項目 | 既定 | 内容 |
|---|---|---|
| `related_batch_hour_utc` | 18 | 関連記事の毎日の一括計算を始める時刻（UTC の時。18 は日本時間の3時） |

## 管理画面（`[admin]`）

| 項目 | 内容 |
|---|---|
| `dist_dir` | 管理画面のファイルのディレクトリ。なければ管理画面を配信しません（管理 API は使えます） |

## 組み込みJS（`[scripts]`）

| 項目 | 既定 | 内容 |
|---|---|---|
| `memory_limit_bytes` | 134217728（128MB） | 1回の実行のメモリの上限 |
| `time_limit_ms` | 5000 | 同期の実行（URL生成関数、保存前と公開前のフック）の時間の上限 |
| `async_time_limit_ms` | 30000 | 非同期のフックの時間の上限 |

## 監査ログ（`[audit]`、`[audit_export]`）と通知（`[alerts]`）

[監視と監査ログ](monitoring.md) を見てください。
