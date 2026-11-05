# new-cms

ブログ向けのヘッドレスCMSです。セルフホスト型のオープンソース（Apache-2.0）で、1つのインスタンスの中に複数のワークスペースを持てます。公式のクラウドは運営しません。

A headless CMS for blogs: self-hosted, open source (Apache-2.0), with several workspaces in one instance. There is no official hosted service.

> **v1.0 の公開の前です。** いまは Issue での報告だけを受け付けています（コントリビューションは v1.0 の公開の後に、DCO で受け付けます）。脆弱性の報告は、公開の Issue ではなく、`SECURITY.md` の窓口へお願いします。
>
> **Pre-v1.0.** Only issue reports are accepted for now; contributions will be accepted under the DCO after v1.0. Please report vulnerabilities privately as described in `SECURITY.md`, not in public issues.

## できること

- ユーザーが定義するスキーマ（テンプレート）と、Tiptap の構造化 JSON の本文
- 下書き・公開・版・予約公開・レビュー依頼・訂正として公開・鮮度（最終確認日と確認期限）
- GraphQL の配信 API（Persisted Queries と GET 配信）。キャッシュは利用者のサイトの側で持つ
- 公開 URL の生成とリダイレクト、訂正フィード、AI 向けの素材（記事の Markdown 版・`llms.txt`・学習拒否）
- サイト内検索（ベクトル検索とキーワード検索）、関連記事
- 公開済みの記事だけに基づいて、出典付きで答えるチャットボットの回答 API（判定先行のストリーミング）
- 編集者向けの AI 支援（矛盾の検知・見直しの手がかり・書くべき記事の構成案）
- LLM と埋め込みの呼び出しは、ワークスペースごとの予算（トークン数）の範囲でだけ行う

## 構成

| 部分 | 技術 | 場所 |
|---|---|---|
| サーバー | Rust | `server/` |
| 管理画面 | Nuxt（SPA。サーバーが配信する） | `admin/` |
| CLI | Node.js | `cli/` |
| 公式の連携部品 | Nuxt モジュール | `packages/nuxt/` |
| 共有のパッケージ | TypeScript | `packages/`（本文の部品の定義・変換器・ライブプレビュー） |
| DB | PostgreSQL 16 以上（pgvector、pg_bigm または PGroonga） | 公式の Docker Compose は `deploy/` |

## 動かす

公式の Docker Compose で動かします。手順は `deploy/README.md` にあります。

## 資料

| 資料 | 内容 |
|---|---|
| `docs/decisions/` | 現在有効な設計の決定（一覧は `docs/decisions/00-index.md`） |
| `docs/invariants.md` | 絶対に破らない約束の一覧。約束ごとに `tests/invariants/` に自動テストがある |
| `docs/history/` | 決定の経緯 |

## ライセンス

Apache License 2.0（`LICENSE`）。依存ライブラリのライセンスは、Apache-2.0 と両立するものだけを使います（`deny.toml`）。
