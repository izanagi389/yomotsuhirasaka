// 評価用の CMS（setup.ts で準備したもの）で、データセットの質問を3つの方法で検索し、指標を results/<モデル>.json に書く。
//
//   node tools/search-eval/src/evaluate.ts
//
// - 記事の単位: Recall@5・Recall@10・MRR@10・nDCG@10（関連の度合いは queries.json の grade）
// - セクションの単位: 正解に見出しがある質問で、正解の記事が10位以内に出たとき、そのチャンクが正解の見出しのセクションだった割合
//   （サイト内検索は1つの記事からいちばん合うチャンクを1件だけ返すので、そのチャンクのパンくずに正解の見出しがあるかを見る）
// 質問文と結果は手元のファイルにだけ書く（results/ はリポジトリに入れない）。

import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { Cms, type SearchHit, type SearchMode } from "./cms.ts";
import { loadDataset, type EvalQuery } from "./dataset.ts";
import { mean, ndcgAt, recallAt, reciprocalRankAt } from "./metrics.ts";
import { BASE_URL, EVAL_DIR, loadState, type WorkspaceState } from "./state.ts";

const MODES: SearchMode[] = ["keyword", "vector", "hybrid"];
const DEPTH = 10;

export interface QueryResult {
  id: string;
  kind: string;
  /** 返した記事の slug（順に）と、その関連の度合い。 */
  slugs: string[];
  grades: number[];
  recall5: number;
  recall10: number;
  rr10: number;
  ndcg10: number;
  /** 正解のセクションがある質問だけ: 正解の記事が出たか、出たときにセクションも合っていたか。 */
  section?: { articleFound: boolean; sectionMatched: boolean; returnedBreadcrumbs: string[][] };
}

export interface ModeResult {
  dataset: string;
  mode: SearchMode;
  queries: QueryResult[];
}

const normalize = (s: string): string => s.normalize("NFKC").replace(/\s+/g, "");

function judge(query: EvalQuery, hits: SearchHit[], slugs: Record<string, string>): QueryResult {
  const returned = hits.map((h) => slugs[h.entryId] ?? `unknown:${h.entryId}`);
  const gradeOf = (slug: string): number => Math.max(0, ...query.relevant.filter((r) => r.slug === slug).map((r) => r.grade));
  const grades = returned.map(gradeOf);
  const relevantGrades = query.relevant.map((r) => r.grade);
  const result: QueryResult = {
    id: query.id,
    kind: query.kind,
    slugs: returned,
    grades,
    recall5: recallAt(5, grades, relevantGrades.length),
    recall10: recallAt(DEPTH, grades, relevantGrades.length),
    rr10: reciprocalRankAt(DEPTH, grades),
    ndcg10: ndcgAt(DEPTH, grades, relevantGrades),
  };
  const withHeading = query.relevant.filter((r) => r.heading !== undefined);
  if (withHeading.length > 0) {
    const found = withHeading.map((r) => hits[returned.indexOf(r.slug)]).filter((h) => h !== undefined);
    // チャンクが正解の見出しの下（さらに下の見出しを含む）にあれば合っているとする。パンくずの先頭は記事のタイトル
    const inSection = (h: SearchHit, heading: string): boolean => h.breadcrumb.slice(1).some((b) => normalize(b) === normalize(heading));
    const matched = found.some((h) => withHeading.some((r) => inSection(h, r.heading!)));
    result.section = { articleFound: found.length > 0, sectionMatched: matched, returnedBreadcrumbs: found.map((h) => h.breadcrumb) };
  }
  return result;
}

async function runMode(cms: Cms, dataset: string, ws: WorkspaceState, mode: SearchMode): Promise<ModeResult> {
  const { locale, queries } = loadDataset(join(EVAL_DIR, "data", dataset, "queries.json"));
  const results: QueryResult[] = [];
  for (const query of queries) {
    const r = await cms.search(ws.id, ws.readKey, ws.queryId, { text: query.text, locale, first: DEPTH, mode });
    const used = mode === "keyword" ? r.keywordSearch : r.vectorSearch;
    if (used !== "AVAILABLE") throw new Error(`${dataset}/${mode}: the search method is ${used}`);
    results.push(judge(query, r.hits, ws.slugs));
  }
  return { dataset, mode, queries: results };
}

export function summarize(queries: QueryResult[]) {
  const sections = queries.flatMap((q) => (q.section?.articleFound ? [q.section.sectionMatched ? 1 : 0] : []));
  return {
    count: queries.length,
    recall5: mean(queries.map((q) => q.recall5)),
    recall10: mean(queries.map((q) => q.recall10)),
    mrr10: mean(queries.map((q) => q.rr10)),
    ndcg10: mean(queries.map((q) => q.ndcg10)),
    sectionMatch: sections.length === 0 ? null : mean(sections),
    sectionCount: sections.length,
  };
}

async function main(): Promise<void> {
  const state = loadState();
  const cms = new Cms(BASE_URL);
  const results: ModeResult[] = [];
  for (const [dataset, ws] of Object.entries(state.workspaces)) {
    for (const mode of MODES) {
      const r = await runMode(cms, dataset, ws, mode);
      const s = summarize(r.queries);
      console.log(`${state.model} ${dataset} ${mode}: nDCG@10 ${s.ndcg10.toFixed(3)} MRR@10 ${s.mrr10.toFixed(3)} R@10 ${s.recall10.toFixed(3)}`);
      results.push(r);
    }
  }
  mkdirSync(join(EVAL_DIR, "results"), { recursive: true });
  writeFileSync(join(EVAL_DIR, "results", `${state.model}.json`), JSON.stringify({ model: state.model, results }, null, 2));
}

if (import.meta.main) await main();
