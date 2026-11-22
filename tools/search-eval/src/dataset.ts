// 評価のデータの形（data/<名前>/queries.json）。データ自体はリポジトリに入れない（.gitignore）。

import { readFileSync } from "node:fs";

/** 質問の正解の1つ。記事は slug で、セクションは見出しの文字列で指す（アンカーIDは取り込みのときにサーバーが決めるため）。 */
export interface Relevant {
  slug: string;
  /** 2 は正解、1 は関連あり（同じ話題の別の記事など）。 */
  grade: 1 | 2;
  /** 正解のセクションの見出し（検索結果のパンくずの最後と比べる）。なければ記事の単位だけで数える。 */
  heading?: string;
}

export interface EvalQuery {
  id: string;
  text: string;
  /** 質問の種類（exact・paraphrase・vague など）。種類ごとの集計に使う。 */
  kind: string;
  relevant: Relevant[];
}

export interface Dataset {
  name: string;
  locale: string;
  queries: EvalQuery[];
}

export function loadDataset(path: string): Dataset {
  const parsed = JSON.parse(readFileSync(path, "utf8")) as Dataset;
  if (!Array.isArray(parsed.queries) || parsed.queries.length === 0) throw new Error(`${path}: no queries`);
  for (const q of parsed.queries) {
    if (q.relevant.length === 0) throw new Error(`${path}: query ${q.id} has no relevant entries`);
  }
  return parsed;
}
