# エラーコードの一覧

## 管理API と配信API の入口のエラー

本文は `{"error": "<コード>", "message": "<英語の説明>"}` です。プログラムで判別するときはコードを見ます。管理画面はコードから日本語の文言を出します。

状態コードの考え方: 認証の失敗は 401、権限と CSRF は 403、見つからないものは 404、検証の失敗は 4xx、回数の制限は 429（`Retry-After` ヘッダー付き）、下書きのリビジョン番号の食い違いは 409 です。

### 認証とセッション

| コード | 状態 | 意味 |
|---|---|---|
| `unauthenticated` | 401 | ログインしていません |
| `invalid_credentials` | 401 | メールアドレスかパスワードが違います（理由は区別しません） |
| `invalid_mfa_code` | 401 | 多要素認証のコードが違います |
| `invalid_passkey` | 422 | パスキーを確認できません |
| `no_passkeys` | 409 | パスキーが登録されていません |
| `passkey_already_registered` | 409 | 登録済みのパスキーです |
| `too_many_passkeys` | 409 | パスキーの数の上限です |
| `passkeys_unavailable` | 503 | このサーバーではパスキーを使えません |
| `mfa_required` | 403 | 管理の操作に必要な多要素認証を設定していません |
| `mfa_setup_required` | 403 | 全員に多要素認証が必須です。先に設定してください |
| `mfa_already_enabled` | 409 | 認証アプリは設定済みです |
| `mfa_not_enabled` | 409 | 多要素認証を設定していません |
| `reauth_required` | 403 | 再認証が必要です |
| `reauth_failed` | 403 | 再認証に失敗しました |
| `reauth_method_not_allowed` | 422 | この方法では再認証できません。パスキーかいつもの多要素認証を使います |
| `csrf_failed` | 403 | CSRFトークンがありません |
| `csrf_already_issued` | 409 | CSRFトークンは発行済みです。ログインし直します |
| `forbidden_origin` | 403 | 別のオリジンからのリクエストです |
| `session_required` | 403 | 管理画面のセッションでだけ使える操作です |
| `invalid_current_password` | 403 | 今のパスワードが違います |
| `weak_password` | 422 | パスワードの条件を満たしていません |
| `breached_password` | 422 | 漏えいしたパスワードの一覧に載っています |
| `invalid_token` | 400 | リセットや招待のリンクが正しくないか、期限切れです |
| `oidc_not_configured` | 404 | SSOを設定していません |
| `oidc_unavailable` | 503 | 外部への送信の設定に問題があり、SSOを使えません |
| `idp_unreachable` | 502 | IdPに届かないか、IdPの設定に問題があります |
| `too_many_connections` | 429 | 即時通信の接続が多すぎます |

### 権限と所属

| コード | 状態 | 意味 |
|---|---|---|
| `forbidden` | 403 | この操作の権限がありません |
| `not_found` | 404 | 見つかりません（権限のないワークスペースも同じ応答です） |
| `insufficient_scope` | 403 | キーやトークンの範囲が足りません |
| `outside_scope` | 403 | あなたが管理していないワークスペースにも所属している人です。サーバー管理者に依頼します |
| `server_admin_target` | 403 | サーバー管理者は、サーバーのCLIでだけ復旧できます |
| `deploy_scope_forbidden` | 403 | デプロイの範囲のトークンを発行できるのはサーバー管理者だけです |
| `deploy_scope_exclusive` | 422 | デプロイの範囲はほかの範囲と混ぜられません |
| `last_sign_in_method` | 409 | ログインの手段がなくなるので、ひも付けを解除できません |
| `deploy_forbidden` | 403 | デプロイには、多要素認証を設定したサーバー管理者の、デプロイの範囲のトークンが要ります |
| `smtp_required` | 403 | SMTPを設定していないため、招待とリセットの発行ができるのはサーバー管理者だけです |
| `sso_only_user` | 403 | シングルサインオンだけでログインする人です。IdPで回復するか、サーバー管理者に依頼します |
| `account_exists` | 409 | このメールアドレスのアカウントはすでにあります。ログインしてから承諾します |
| `already_member` | 409 | すでに所属しています |
| `last_admin` | 409 | ワークスペースには、少なくとも1人の管理者が要ります |

### 配信API

| コード | 状態 | 意味 |
|---|---|---|
| `invalid_api_key` | 401 | 有効なキーが `Authorization` ヘッダーにありません（別のワークスペースのキーを含みます） |
| `credential_in_url` | 401 | キーやトークンが URL に入っています。送ったものは失効させてください |
| `persisted_query_required` | 400 | 登録済みのクエリの ID を送ってください |
| `persisted_query_not_found` | 400 | その ID のクエリは登録されていません |
| `query_timeout` | 503 | クエリの実行時間が上限を超えました |
| `query_too_large` | 422 | クエリで解決する記事が多すぎます。一覧の件数や参照の深さを減らしてください |
| `delivery_busy` | 503 | 同時に実行するクエリが上限に達しています。少し待って送り直してください |
| `batch_not_supported` | 400 | 1回のリクエストで送れるクエリは1つです |
| `invalid_query` | 422 | クエリが正しくないか、深さや複雑さの上限を超えています |
| `query_limit_reached` | 409 | 登録クエリの数の上限です |
| `query_registration_rate_limited` | 429 | クエリの新規登録が多すぎます。少し待つと続きを登録できます |
| `response_too_large` | 422 | 応答が大きさの上限を超えました |
| `invalid_preview_token` | 401 | プレビュー用トークンが無効か、期限切れです |
| `rate_limited` | 429 | 回数の制限です。`Retry-After` の秒数の後に送ります |

### 回答API（HTTP）

| コード | 状態 | 意味 |
|---|---|---|
| `answer_requires_post` | 405 | 回答は POST で送ります |
| `search_requires_post` | 405 | 検索は POST で送ります（検索語を URL に載せない） |
| `answer_rate_limited` | 429 | キーごとの回答の回数の制限です |
| `visitor_rate_limited` | 429 | 訪問者ごとの回答の回数の制限です |
| `visitor_id_invalid` | 400 | `visitorId` は128文字までの文字列です |
| `answer_test_rate_limited` | 429 | 「回答を試す」の回数の制限です（1分あたり10回） |

### デプロイとスキーマ

| コード | 状態 | 意味 |
|---|---|---|
| `invalid_manifest` | 422 | テンプレートに問題があります |
| `destructive_change` | 409 | 破壊的な変更があります。追加→データの移行→削除の順に分けるか、明示して強行します |
| `breaks_registered_queries` | 409 | 登録済みのクエリが壊れます。サイトのクエリを直して登録し直すか、明示して強行します |
| `schema_not_deployed` | 409 | このワークスペースにはスキーマが適用されていません |
| `schema_migration_pending` | 409 | 移行待ちのため、編集と公開を止めています |

### 記事と公開

| コード | 状態 | 意味 |
|---|---|---|
| `invalid_request` | 400 | リクエストが正しくありません |
| `invalid_entry` | 422 | 記事の内容に受け付けられない部分があります |
| `admin_rate_limited` | 429 | 管理 API へのリクエストが多すぎます。少し待ってやり直します |
| `entry_limit_reached` | 409 | ワークスペースの記事の数の上限です |
| `revision_conflict` | 409 | 他の人が先に保存しました。読み直してからやり直します |
| `entry_locked` | 423 | 他の人が編集中です。引き継いでから保存します |
| `lock_lost` | 409 | 編集が引き継がれたか、期限が切れました |
| `slug_taken` | 409 | 同じロケールの別の記事が同じ値を使っています |
| `url_taken` | 409 | 公開中の別の記事が同じ URL を使っています |
| `alt_text_missing` | 422 | 代替テキストも装飾画像の印もない画像があります |
| `url_function_failed` | 422 | URL生成関数に問題があります。開発者に直してもらいます |
| `hook_failed` | 422 | 公開前のフックに問題があります。開発者に直してもらいます |
| `publish_rejected_by_hook` | 422 | 公開前のフックが公開を止めました |
| `conversion_failed` | 422 | カスタムノードの変換関数に問題があります。開発者に直してもらいます |
| `nothing_to_correct` | 409 | 一度も公開していない記事は、訂正として公開できません |
| `not_published` | 409 | 確認済みを記録できるのは、公開した記事だけです |
| `cannot_schedule_correction` | 422 | 訂正は予約できません |
| `invalid_publish_at` | 422 | 予約の時刻が正しくありません |
| `no_schedule` | 409 | 予約がありません |
| `review_already_open` | 409 | 開いているレビューの依頼がすでにあります |
| `no_open_review` | 409 | 開いているレビューの依頼がありません |
| `invalid_reviewer` | 422 | 見てほしい人は、依頼した人以外の管理者か編集者です |
| `invalid_comment` | 422 | コメントの長さか文字が正しくありません |
| `version_is_published` | 409 | 公開中の版は抹消できません |
| `translation_exists` | 409 | そのロケールの翻訳はすでにあります |
| `no_source` | 409 | 元の記事がありません |

### メディア

| コード | 状態 | 意味 |
|---|---|---|
| `unsupported_media_type` | 415 | JPEG、PNG、GIF、WebP だけを受け付けます（ファイルの中身で判定します） |
| `invalid_image` | 422 | 画像として読めません |
| `image_too_large` | 422 | 画素数が上限を超えています |
| `image_processing_timeout` | 422 | 画像の処理に時間がかかりすぎました |
| `media_processing_busy` | 503 | 処理中の画像が多いため、受け付けられません。少し待ってやり直してください |
| `file_too_large` | 413 | ファイルが上限を超えています |
| `media_quota_exceeded` | 413 | ワークスペースの容量の上限を超えます |
| `media_in_use` | 409 | 記事で使われているメディアです |

### Webhookと監査ログの書き出し

| コード | 状態 | 意味 |
|---|---|---|
| `destination_not_allowed` | 422 | この送信先には送れません（https で、公開のアドレスに解決される URL だけです） |
| `too_many_webhooks` | 409 | Webhook の数の上限です |
| `invalid_hook_secret` | 422 | フックの秘密の名前か値が正しくありません |
| `too_many_hook_secrets` | 409 | フックの秘密の数の上限（50個）です |
| `webhook_test_rate_limited` | 429 | テスト送信が多すぎます。Webhook ごとに1分10回までです |
| `too_many_audit_sinks` | 409 | 書き出し先の数の上限です |

### LLMと予算

| コード | 状態 | 意味 |
|---|---|---|
| `budget_exceeds_ceiling` | 400 | 天井を超えています |
| `llm_override_not_allowed` | 403 | このワークスペースには、LLMの接続設定の上書きが許されていません |
| `invalid_llm_settings` | 422 | LLMの接続設定が正しくありません |
| `unknown_llm_endpoint` | 422 | サーバー管理者が許可した接続先から選びます |
| `unknown_embedding_model` | 422 | サーバー管理者が許可したモデルから選びます |
| `no_next_embedding` | 409 | 切り替え先のモデル（`[llm.next_embedding]`）が設定されていません |
| `no_embedding_switch` | 409 | 切り替え先のモデルがありません |
| `embedding_switch_server_admin` | 403 | サーバーの切り替え先への切り替えを始められるのはサーバー管理者だけです |
| `invalid_answer_settings` | 422 | 回答の設定が正しくありません |
| `invalid_question_log_settings` | 422 | 質問ログの設定が正しくありません |

### そのほか

| コード | 状態 | 意味 |
|---|---|---|
| `internal_error` | 500 | サーバーで問題が起きました。詳細はサーバーのログにだけ残ります |

## GraphQL のエラー（回答API）

回答できなかった理由は、GraphQL のエラーの `extensions.code` で返します。

| コード | 意味 |
|---|---|
| `QUESTION_EMPTY` | 質問が空です |
| `QUESTION_TOO_LONG` | 質問が長すぎます（500文字まで） |
| `QUESTION_REFUSED` | ワークスペースが答えないと決めた質問です。LLM にも埋め込みにも送らず、質問ログにも残しません |
| `LOCALE_NOT_SUPPORTED` | テンプレートのロケールではありません |
| `ANSWER_UNAVAILABLE` | 回答の生成のモデルが設定されていません（`[llm] chat_model`） |
| `ANSWER_BUDGET_EXHAUSTED` | 訪問者向けの回答の予算が尽きています |
| `LLM_UNAVAILABLE` | LLM を呼べませんでした。時間をおいて送り直します |
| `INTERNAL_ERROR` | サーバーで問題が起きました |

回答の最終判定（`status`）はエラーではありません。`ANSWERED`（回答）、`NO_INFORMATION`（記事に答えがない）、`VIOLATION_DETECTED`（本文の途中で規則が破られたので、最終の本文に差し替えた）の3つです。

Nuxt モジュールの中継がブラウザに渡すコードは [回答API](../developer/answer-api.md) にあります。

## CLI の終了コード

`yomi` コマンドの終了コードは [テンプレートとCLI](../developer/templates-and-cli.md) にあります。
