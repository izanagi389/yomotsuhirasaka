// JQaRA（hotchpotch/JQaRA の test。CC-BY-SA 4.0）の小さな部分集合を、取り込みの CLI で入れられる形にする。
//
//   node tools/search-eval/src/jqara.ts [質問の数（既定 50）]
//
// - 質問ごとに候補の文書が100件あり、正解（label = 1）が付いている。先頭から質問の数だけ取る（毎回同じ部分集合）
// - 文書は1件を1記事の Markdown にする（data/jqara/docs/part-N。1回の取り込みの上限に合わせて分ける）。同じ文書が複数の質問の候補になっていれば1件にまとめる
// - 正解の組は data/jqara/queries.json（形は dataset.ts）
// - 読むのは Hugging Face の dataset viewer の API（JSON）。依存を足さないため、parquet は読まない

import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { Dataset, EvalQuery } from "./dataset.ts";

const ROWS_API = "https://datasets-server.huggingface.co/rows?dataset=hotchpotch/JQaRA&config=default&split=test";
const CANDIDATES_PER_QUESTION = 100;
const PAGE_SIZE = 100;
const DEFAULT_QUESTIONS = 50;
const MAX_ATTEMPTS = 6;
const RETRY_DELAY_MS = 10_000;
/** dataset viewer の回数制限に当たらないよう、ページの間に空ける時間。 */
const PAGE_INTERVAL_MS = 1_500;
/** 1回の取り込みで送れる記事は2,000件まで（server/src/http/admin_import.rs）なので、フォルダを分ける。 */
const DOCS_PER_FOLDER = 1_500;

interface Row {
  q_id: string;
  passage_row_id: string;
  label: number;
  text: string;
  title: string;
  question: string;
}

const OUT = join(import.meta.dirname, "..", "data", "jqara");

async function fetchRows(offset: number): Promise<Row[]> {
  for (let attempt = 1; ; attempt++) {
    const res = await fetch(`${ROWS_API}&offset=${offset}&length=${PAGE_SIZE}`);
    if (res.ok) {
      const body = (await res.json()) as { rows: { row: Row }[] };
      return body.rows.map((r) => r.row);
    }
    // dataset viewer はときどき 502・429 を返すので、間を空けて何度か試す
    const retryable = res.status === 429 || res.status >= 500;
    if (!retryable || attempt >= MAX_ATTEMPTS) throw new Error(`dataset viewer: status ${res.status} at offset ${offset}`);
    await new Promise((r) => setTimeout(r, RETRY_DELAY_MS * attempt));
  }
}

/** Markdown の記号を文字として扱わせる（文書は地の文なので、書式として解釈させない）。 */
const escapeMarkdown = (s: string): string => s.replace(/[\\`*_[\]<>#|~!]/g, (c) => `\\${c}`);

const slugOf = (passageRowId: string): string => `p${passageRowId}`;

function markdown(row: Row): string {
  const front = [
    "---",
    `title: ${JSON.stringify(row.title)}`,
    `slug: ${JSON.stringify(slugOf(row.passage_row_id))}`,
    `url: ${JSON.stringify(`https://jqara.invalid/p/${row.passage_row_id}`)}`,
    "---",
  ];
  return `${front.join("\n")}\n\n${escapeMarkdown(row.text)}\n`;
}

async function main(): Promise<void> {
  const questions = Number(process.argv[2] ?? DEFAULT_QUESTIONS);
  if (!Number.isInteger(questions) || questions <= 0) throw new Error("the number of questions must be a positive integer");
  const rows: Row[] = [];
  for (let offset = 0; offset < questions * CANDIDATES_PER_QUESTION; offset += PAGE_SIZE) {
    rows.push(...(await fetchRows(offset)));
    await new Promise((r) => setTimeout(r, PAGE_INTERVAL_MS));
  }

  const byQuestion = new Map<string, Row[]>();
  for (const row of rows) byQuestion.set(row.q_id, [...(byQuestion.get(row.q_id) ?? []), row]);
  const queries: EvalQuery[] = [...byQuestion.values()].slice(0, questions).map((candidates) => ({
    id: candidates[0]!.q_id,
    text: candidates[0]!.question,
    kind: "jqara",
    relevant: candidates.filter((c) => c.label === 1).map((c) => ({ slug: slugOf(c.passage_row_id), grade: 1 as const })),
  }));
  const dataset: Dataset = { name: "jqara", locale: "ja", queries: queries.filter((q) => q.relevant.length > 0) };

  rmSync(join(OUT, "docs"), { recursive: true, force: true });
  const written = new Set<string>();
  for (const row of rows) {
    if (written.has(row.passage_row_id)) continue;
    const folder = join(OUT, "docs", `part-${Math.floor(written.size / DOCS_PER_FOLDER) + 1}`);
    written.add(row.passage_row_id);
    mkdirSync(folder, { recursive: true });
    writeFileSync(join(folder, `${slugOf(row.passage_row_id)}.md`), markdown(row));
  }
  writeFileSync(join(OUT, "import.json"), JSON.stringify({ contentType: "post", originalUrlKey: "url" }, null, 2));
  writeFileSync(join(OUT, "queries.json"), JSON.stringify(dataset, null, 2));
  console.log(`questions: ${dataset.queries.length}, documents: ${written.size}`);
}

await main();
