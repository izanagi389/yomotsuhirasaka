// results/*.json（evaluate.ts の出力）をまとめて、results/report.md に書く。
//
//   node tools/search-eval/src/report.ts

import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { summarize, type ModeResult, type QueryResult } from "./evaluate.ts";
import { EVAL_DIR } from "./state.ts";

interface ModelResults {
  model: string;
  results: ModeResult[];
}

const RESULTS = join(EVAL_DIR, "results");
const pct = (v: number | null): string => (v === null ? "-" : v.toFixed(3));

function table(rows: { label: string; queries: QueryResult[] }[]): string[] {
  const lines = ["| 組み合わせ | 件数 | Recall@5 | Recall@10 | MRR@10 | nDCG@10 | セクション一致 |", "|---|---:|---:|---:|---:|---:|---:|"];
  for (const { label, queries } of rows) {
    const s = summarize(queries);
    const section = s.sectionMatch === null ? "-" : `${pct(s.sectionMatch)}（${s.sectionCount}件）`;
    lines.push(`| ${label} | ${s.count} | ${pct(s.recall5)} | ${pct(s.recall10)} | ${pct(s.mrr10)} | ${pct(s.ndcg10)} | ${section} |`);
  }
  return lines;
}

/** キーワード検索はモデルによらないので、最初のモデルの分だけを載せる。 */
function combinations(all: ModelResults[], dataset: string, filter: (q: QueryResult) => boolean) {
  const rows: { label: string; queries: QueryResult[] }[] = [];
  all.forEach((m, i) => {
    for (const r of m.results.filter((r) => r.dataset === dataset)) {
      if (r.mode === "keyword" && i > 0) continue;
      rows.push({ label: r.mode === "keyword" ? "keyword" : `${r.mode} / ${m.model}`, queries: r.queries.filter(filter) });
    }
  });
  return rows;
}

function misses(all: ModelResults[], dataset: string): string[] {
  const lines = ["| 組み合わせ | 10位以内に正解がなかった質問 |", "|---|---|"];
  for (const { label, queries } of combinations(all, dataset, () => true)) {
    lines.push(`| ${label} | ${queries.filter((q) => q.rr10 === 0).map((q) => q.id).join(", ") || "なし"} |`);
  }
  return lines;
}

function main(): void {
  const all = readdirSync(RESULTS)
    .filter((f) => f.endsWith(".json"))
    .map((f) => JSON.parse(readFileSync(join(RESULTS, f), "utf8")) as ModelResults);
  const datasets = [...new Set(all.flatMap((m) => m.results.map((r) => r.dataset)))];
  const out: string[] = ["# 検索の精度の評価", ""];
  for (const dataset of datasets) {
    out.push(`## ${dataset}`, "", ...table(combinations(all, dataset, () => true)), "");
    const kinds = [...new Set(all[0]!.results.find((r) => r.dataset === dataset)!.queries.map((q) => q.kind))];
    if (kinds.length > 1) {
      for (const kind of kinds) out.push(`### 質問の種類: ${kind}`, "", ...table(combinations(all, dataset, (q) => q.kind === kind)), "");
    }
    out.push("### 外した質問", "", ...misses(all, dataset), "");
  }
  writeFileSync(join(RESULTS, "report.md"), out.join("\n"));
  console.log(out.join("\n"));
}

main();
