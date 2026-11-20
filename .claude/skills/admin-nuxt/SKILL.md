---
name: admin-nuxt
description: admin/（Nuxt の管理画面。SPA）のコードを書く・直すときの決まり。データの取得、API の呼び出し、状態、文言、テストとチェックの実行方法。admin/ の .vue・.ts を変更する作業で読む。
---

# 管理画面（admin/）の決まり

全体の基準は `code-quality` スキル、禁止事項は CLAUDE.md に従う。

## 前提

- Nuxt 4 の **SPA**（`ssr: false`）。`nuxt generate` の出力を Rust のサーバーが `/admin/` で配信する。本番で Node.js は動かさない。SSR を前提にした書き方を持ち込まない。
- パッケージは **npm workspaces**（ルートの `package.json`）。pnpm などを使わない。
- 新しい依存（Pinia、テスト用のライブラリなど）は追加しない。必要なら CLAUDE.md の手順で承認を求める。

## 書き方

- コンポーネントは `<script setup lang="ts">` と Composition API で書く。TypeScript は strict。
- **API の呼び出しは `useApi`** を通す（CSRF トークン、再認証のダイアログを扱っている）。`$fetch` や `fetch` を直接呼ばない。
- **データの取得は `useAsyncData`。** 複数の画面や常に表示されている部品が**同じキーで共有する**データは `useFreshAsyncData` を使う（画面を移っても古いデータが残る問題への対策。`composables/useFreshAsyncData.ts`）。
- 状態は、その画面だけのものは `ref` / `reactive`、画面をまたぐものは `useState`。Pinia は使わない。
- 画面の文言は翻訳ファイル（`admin/i18n/`、日本語・英語）に置き、`useI18n` で使う。コードに直接書かない。
- `v-html` を使わない。`eval` などの動的なコードの評価も使わない（ESLint で禁止している）。
- shadcn-vue の部品（`components/ui/`）は自動では読み込まれない。使う画面で明示して import する。
- 同じ処理が既に `composables/` や `utils/` にないか探してから書く。

## チェック

```bash
npm run lint -w @yomotsuhirasaka/admin
npm run typecheck -w @yomotsuhirasaka/admin
```

- 管理画面だけの単体テストの仕組みはない。振る舞いの約束は `tests/invariants/`（管理API を外から検査する）で守る。
- 画面の見た目や操作を変えたときは、可能なら手元で起動して確認し、確認できなかったら報告する。
