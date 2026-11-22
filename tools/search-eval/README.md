# 検索の精度の評価

サイト内検索（キーワード・ベクトル・ハイブリッド）が、質問に対して正しい記事とセクションを上位に返すかを測ります。開発用の道具で、本番では使いません。CI でも動かしません（本物の埋め込みモデルを使い、時間がかかるため）。

- 埋め込みは、手元の [Ollama](https://ollama.com/) で作ります。外部の API を呼ばないので、費用はかかりません。
- 評価用の CMS は、普段の CMS（`deploy/`）とは別の Docker Compose のプロジェクト（`yomi-eval`）で動かします。データは `work/` に置き、DB は tmpfs です。
- 評価のデータ（`data/`）、評価用の CMS のデータ（`work/`）、結果（`results/`）は、リポジトリに入れません。記事の本文や質問文を含むためです。

## 用意するもの

- Docker と Ollama。埋め込みモデルを取り込んでおく（例: `ollama pull bge-m3`）。
- 評価用のサーバーのイメージ（リポジトリのルートで）

  ```bash
  docker build -f deploy/server.Dockerfile -t yomotsuhirasaka/server:eval .
  ```

- 取り込みの CLI（`npm run build -w cli`）
- Node 24 以上（型の除去で TypeScript をそのまま動かす。依存はない）

## データ

`data/<データセット>/queries.json` に、質問と正解の組を置きます（形は `src/dataset.ts`）。

- `own`: 自分の記事の質問。記事は Markdown のフォルダから取り込む（`setup.ts --own`）。
- `jqara`: [JQaRA](https://huggingface.co/datasets/hotchpotch/JQaRA)（CC-BY-SA 4.0）の test の部分集合。`node tools/search-eval/src/jqara.ts [質問の数]` で、質問と文書を `data/jqara/` に書き出す（既定は50問、文書は約5,000件）。

正解の度合いは、2 が正解、1 が関連あり（同じ話題の別の記事など）です。`heading` を書いた正解は、セクションの単位でも数えます。

## 実行

モデルごとに、評価用の CMS を作り直して測ります（DB を作り直すので、前のモデルのデータは残りません）。

```bash
tools/search-eval/init.sh bge-m3 1024
node tools/search-eval/src/setup.ts --own <Markdown のフォルダ> --own-config <取り込みの設定>
node tools/search-eval/src/evaluate.ts
```

別のモデルでも同じ3つを行い、最後にまとめます。

```bash
node tools/search-eval/src/report.ts
```

`results/report.md` に、データセットごと・質問の種類ごとの表と、外した質問の一覧が出ます。

終わったら止めます。

```bash
docker compose -f tools/search-eval/compose.yml down
```

## 指標

| 指標 | 意味 |
|---|---|
| Recall@5・Recall@10 | 正解の記事のうち、上位5件・10件に入った割合 |
| MRR@10 | 最初の正解の順位の逆数の平均（10位までに正解がなければ0） |
| nDCG@10 | 正解の度合いと順位の両方を見た点数（1が最良） |
| セクション一致 | 正解のセクションがある質問で、正解の記事が10位以内に出たとき、そのチャンクが正解のセクションだった割合 |

- サイト内検索は、1つの記事からいちばん合うチャンクを1件だけ返します。セクション一致は、そのチャンクのパンくず（タイトルを除く）に正解の見出しがあるかで数えます。正解の見出しの下の小見出しのチャンクも、合っているとします。
- 回答 API が根拠を選ぶ検索（記事ごとにまとめずにチャンクを並べる）は、配信 API から直接は呼べないので、ここでは測りません。
- キーワード検索は、検索語を空白で区切った語をすべて含むチャンクだけを返します（`server/src/search/query.rs`）。文の形の質問では、当たらないことがあります。
