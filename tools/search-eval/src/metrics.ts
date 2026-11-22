// 検索の精度の指標（Recall@k・MRR@k・nDCG@k）。どれも1つの質問の値で、平均は呼び出し側で取る。
//
// - grades: 返した順に、その結果の関連の度合い（0 は関係ない。1・2 は関連あり、2 がより強い）
// - relevantGrades: その質問の正解すべての関連の度合い（返したかどうかによらない）

/** 上位 k 件に入った正解の数 ÷ 正解の数。正解がなければ 0。 */
export function recallAt(k: number, grades: number[], relevantCount: number): number {
  if (relevantCount === 0) return 0;
  const found = grades.slice(0, k).filter((g) => g > 0).length;
  return Math.min(found, relevantCount) / relevantCount;
}

/** 最初の正解の順位の逆数（上位 k 件になければ 0）。 */
export function reciprocalRankAt(k: number, grades: number[]): number {
  const index = grades.slice(0, k).findIndex((g) => g > 0);
  return index === -1 ? 0 : 1 / (index + 1);
}

/** nDCG@k。利得は 2^g − 1、割引は log2(順位 + 1)。 */
export function ndcgAt(k: number, grades: number[], relevantGrades: number[]): number {
  const dcg = (gs: number[]): number => gs.slice(0, k).reduce((sum, g, i) => sum + (2 ** g - 1) / Math.log2(i + 2), 0);
  const ideal = dcg([...relevantGrades].sort((a, b) => b - a));
  return ideal === 0 ? 0 : dcg(grades) / ideal;
}

export const mean = (values: number[]): number => (values.length === 0 ? 0 : values.reduce((a, b) => a + b, 0) / values.length);
