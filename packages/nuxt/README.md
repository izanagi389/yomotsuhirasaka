# @yomotsuhirasaka/nuxt

CMSの公式Nuxtモジュールです。使い方は手順書の [Nuxtモジュール](../../docs/manual/developer/nuxt-module.md) にあります。

## 開発

```sh
npm run build -w @yomotsuhirasaka/nuxt   # dist/ を作る（サイトはこれを読む）
npm test -w @yomotsuhirasaka/nuxt
```

`playground/` は確かめ用のサイトです（`YOMI_URL`・`YOMI_WORKSPACE_ID`・`YOMI_REGISTER_KEY`・`NUXT_YOMI_READ_KEY` を設定して `npx nuxt generate`）。
