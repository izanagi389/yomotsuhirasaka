# 決定事項の一覧

このフォルダには、**現在有効な決定だけ**を書きます。決定の経緯や見送った案は `docs/history/` にあります。食い違う場合は、このフォルダが正です。

各項目の末尾の `(Qnn)` は、判断記録の質問番号です。

| ファイル | 内容 |
|---|---|
| `01-architecture.md` | 全体構成、マルチテナント、マイルストーン、未決事項 |
| `02-schema-and-delivery-api.md` | スキーマ定義、移行、GraphQL配信、Persisted Queries |
| `03-content-and-editing.md` | 本文（Tiptap）、アンカー、版、レビュー、予約公開、プレビュー、同時編集、取り込み、管理画面の体験 |
| `04-hooks.md` | 組み込みJSランタイム、フック、ホスト関数 |
| `05-urls-and-redirects.md` | 公開URL、URLの変更履歴、リダイレクト |
| `06-corrections-and-freshness.md` | 訂正履歴、鮮度（最終確認日と確認期限）、訂正フィード |
| `07-ai-delivery.md` | AI向け素材（Markdown版、`llms.txt`）、学習拒否、出し分け用メタデータ |
| `08-search-and-related.md` | 検索の基盤、チャンク分割、埋め込み、関連記事 |
| `09-qa-chatbot.md` | 質問応答（チャットボット） |
| `10-llm-and-budget.md` | LLMの接続設定、予算、編集者向けのAI支援 |
| `11-permissions-and-audit.md` | ロールと権限、ワークスペースと利用者、監査ログ |
| `12-dev-environment.md` | 開発環境、擬似LLM |
| `13-security.md` | 脅威モデル、ログイン、OIDC、セッション、デプロイ、SQLとDBの権限、鍵、GraphQLの制限、外部への送信、メディア、本文、チャットボットへの悪用、依存ライブラリ、運用 |
