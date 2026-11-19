// 編集者向けのAI支援（執筆支援を含む）の約束: INV-EDIT-02、INV-BUD-03（M5、10-llm-and-budget.md「編集者向けのAI支援」「予算」、
// Q69, Q70、2026-11-17 の変更）
//
// - 支援の結果は提案として返すだけで、下書きは編集者が「当てる」まで変わらない。当てるのは管理画面が下書きの保存として行う。
//   なので、支援のAPIを何度呼んでも、下書きの内容（タイトル・本文・項目）とリビジョン番号は変わらない（INV-EDIT-02）。
// - 支援のための LLM は、ワークスペースの「編集者向けの支援」の予算（入力・出力別）の範囲でだけ呼ぶ。同時に何件始めても、
//   届いた入力・出力のトークン数の合計は上限を超えない（INV-BUD-03）。予算が尽きた後は LLM を呼ばない。
// - 舞台は専用のインスタンスと専用の擬似LLMのキー（support/world.ts の createAnswerWorld）。テンプレートは、タイトルと要約の案を
//   当てる項目を指定したもの（WRITING_ASSIST_TEMPLATE）。代替テキストの案のために、画像を読むモデルも設定する
//   （擬似LLMは画像を読まず、決まった案を返す）。
// - 擬似LLMは回答の組み立て方 `fixed` で、機能ごとに決まった出力を返す（format.json の形）。出力は日本語にする
//   （日本語の記事では、日本語でない出力は取り直した後に出さないため）。
// - 対照: 下書きを変えないことの確認では、どの支援も LLM を呼んで提案を1つ以上出したこと（当てれば下書きが変わる提案が
//   実際に出ていること）を確かめる。予算の確認では、予算の範囲では LLM が呼ばれることを確かめる。
// - 検索語はマスク（10桁以上の数字の並び → [number]。Q169）で変わらないよう、数字を含めない。

import { afterAll, expect } from "vitest";
import { inv } from "./support/inv.js";
import { asSession, driver, type Draft, type EntryAssistKind, type EntryAssistResult } from "./support/driver.js";
import { doc, heading, paragraph } from "./support/body.js";
import { pngImage } from "./support/images.js";
import {
  altTextOutput,
  chatMessageTexts,
  chatRequests,
  mockLlm,
  mockUsageTokens,
  noConflictOutput,
  proofreadOutput,
  structureOutput,
  summaryOutput,
  titleOutput,
} from "./support/mock-llm.js";
import { WRITING_ASSIST_TEMPLATE } from "./support/template.js";
import { createAnswerWorld, marker, sleep, type AnswerWorkspace, type AnswerWorld } from "./support/world.js";

let worldPromise: Promise<AnswerWorld> | null = null;
const world = (): Promise<AnswerWorld> =>
  (worldPromise ??= createAnswerWorld({ template: WRITING_ASSIST_TEMPLATE, llm: { visionModel: mockLlm.chatModel } }));

afterAll(async () => {
  if (worldPromise) await (await worldPromise).stop();
});

/** 支援の予算の、十分に大きい天井。 */
const LARGE = 10_000_000;

/** 数字を含まない一意な目印（マスクで変わらないようにする）。 */
const wordMarker = (label: string): string => marker(label).replace(/\d/g, (d) => "ghijklmnop"[Number(d)]!);

/** ボタンで始める執筆支援（見直しの手がかりは確認期限切れの記事が対象なので、ここでは扱わない）。 */
const WRITING_KINDS: readonly Exclude<EntryAssistKind, "stale">[] = ["structure", "proofread", "summary", "title"];

/** 校正で直させる語句（本文の段落に1か所だけ入れる）。 */
const TYPO = "せつめいしま";
const FIXED = "説明します";

/** 機能ごとの擬似LLMの出力（日本語）。 */
const OUTPUT: Record<Exclude<EntryAssistKind, "stale"> | "altText", string> = {
  structure: structureOutput("見出しの順番を入れ替えると、読み手が手順を追いやすくなります。"),
  proofread: proofreadOutput([
    { kind: "error", from: TYPO, to: FIXED },
    { kind: "style", from: `料金と提供時間を${TYPO}。`, to: `料金と提供時間を${FIXED}。` },
  ]),
  summary: summaryOutput("料金と提供時間の決まりを、手順とあわせて紹介する記事です。"),
  title: titleOutput("料金と提供時間の決まりのまとめ"),
  altText: altTextOutput("料金表を示す図"),
};

async function enableAssist(ws: AnswerWorkspace): Promise<void> {
  const { yomi } = await world();
  await yomi.reauthenticate(ws.adminSession, ws.admin);
  await yomi.setAssistSettings(ws.adminSession, ws.id, { enabled: true });
}

/** 支援の予算（入力・出力別。月・日。null は上限なし）を決める。 */
async function setBudget(
  ws: AnswerWorkspace,
  input: { month: number | null; day: number | null },
  output: { month: number | null; day: number | null },
): Promise<void> {
  const { yomi } = await world();
  await yomi.reauthenticate(ws.adminSession, ws.admin);
  const pair = (b: { month: number | null; day: number | null }) => ({ tokensPerMonth: b.month, tokensPerDay: b.day });
  const res = await yomi.setAssistBudgetRaw(asSession(ws.adminSession), ws.id, { input: pair(input), output: pair(output) });
  expect(res.status, `管理者による支援の予算の設定（${res.status}）: ${res.text}`).toBeLessThan(300);
}

/** 本文に画像（代替テキストなし）と、校正で直させる語句を含む下書き。topic は近い記事と共有する言葉。 */
async function draftWithImage(ws: AnswerWorkspace, topic: string, label: string): Promise<{ id: string; mediaId: string }> {
  const { yomi } = await world();
  const media = await yomi.uploadMedia(ws.editorSession, ws.id, { filename: "chart.png", bytes: pngImage(32, 24), contentType: "image/png" });
  const own = wordMarker(label);
  const body = doc(
    heading("料金について"),
    paragraph(`${topic} の料金と提供時間を${TYPO}。${own} の手順もあわせて案内します。`),
    driver.body.image({ mediaId: media.id }),
    heading("手順"),
    paragraph("申し込みの手順は、窓口と画面のどちらからでも同じです。"),
  );
  const { id } = await yomi.createEntry(ws.editorSession, ws.id, {
    title: marker(`${label}-title`),
    body,
    fields: { summary: "まだ書いていない要約" },
  });
  return { id, mediaId: media.id };
}

/** 支援が running でなくなるまで待つ。 */
async function settled(read: () => Promise<EntryAssistResult>, what: string): Promise<EntryAssistResult> {
  const deadline = Date.now() + 180_000;
  for (;;) {
    const r = await read();
    if (r.state !== "running") return r;
    expect(Date.now() < deadline, `${what}が終わらない`).toBe(true);
    await sleep(1);
  }
}

/** 記事が配信API（検索）に出るまで待つ。 */
async function waitSearchable(ws: AnswerWorkspace, entryId: string, word: string): Promise<void> {
  const { yomi } = await world();
  const deadline = Date.now() + 120_000;
  for (;;) {
    const r = await yomi.search(ws.id, ws.readKey.secret, ws.searchQuery, { text: word, first: 50 });
    if (r.hits.some((h) => h.entryId === entryId)) return;
    expect(Date.now() < deadline, "近い記事が配信APIに出ない").toBe(true);
    await sleep(1);
  }
}

/** 意味の近い、公開中の別の記事（矛盾の検知と構成の確認の「近い記事」）。 */
async function publishNeighbor(ws: AnswerWorkspace, topic: string): Promise<void> {
  const { yomi } = await world();
  const other = await yomi.createEntry(ws.editorSession, ws.id, {
    title: marker("neighbor"),
    body: doc(paragraph(`${topic} の料金は月ごとに決まり、提供時間は平日の朝から夕方までです。`)),
  });
  await yomi.publishEntry(ws.editorSession, ws.id, other.id);
  await waitSearchable(ws, other.id, topic);
}

/** 下書きの、比べる部分（内容とリビジョン番号）。 */
const snapshot = (d: Draft) => ({ revision: d.revision, title: d.title, slug: d.slug, body: d.body, fields: d.fields });

inv(
  "inv_edit_02",
  "編集者向けのAI支援のAPIを呼んでも、下書きの内容とリビジョン番号は変わらない",
  async () => {
    const w = await world();
    const { yomi, llmKey } = w;
    const ws = await w.workspace("edit02", { assistCeiling: LARGE });
    await enableAssist(ws);
    const topic = wordMarker("edit02-topic");
    await publishNeighbor(ws, topic);

    const draft = await draftWithImage(ws, topic, "edit02");
    const before = snapshot(await yomi.getDraft(ws.editorSession, ws.id, draft.id));

    /** 支援を1つ動かして終わるまで待ち、その後も下書きが変わっていないことを確かめる。 */
    const runAndCompare = async (what: string, content: string, start: () => Promise<void>, read: () => Promise<EntryAssistResult>) => {
      await mockLlm.clearRequests(llmKey);
      await mockLlm.setStrategy(llmKey, { kind: "fixed", content });
      await start();
      const result = await settled(read, what);
      // 対照: LLM を呼び、提案を出した（当てれば下書きが変わる提案が、実際に出ている）
      expect(result.state, `${what}が終わらない（${result.state}）`).toBe("done");
      expect(chatRequests(await mockLlm.waitUntilQuiet(llmKey, 3_000)).length, `${what}で LLM が呼ばれない`).toBeGreaterThan(0);
      if (what !== "矛盾の検知") expect(result.items, `${what}の提案が出ない`).toBeGreaterThan(0);
      const after = snapshot(await yomi.getDraft(ws.editorSession, ws.id, draft.id));
      expect(after, `${what}のAPIを呼んだ後に、下書きの内容かリビジョン番号が変わった`).toEqual(before);
    };

    try {
      // 矛盾の検知（公開の確認画面が始めるもの）
      await runAndCompare(
        "矛盾の検知",
        noConflictOutput(),
        () => yomi.startConflictCheck(ws.editorSession, ws.id, draft.id),
        async () => {
          const r = await yomi.conflictCheck(ws.editorSession, ws.id, draft.id);
          return { state: r.state, items: r.conflicts.length };
        },
      );
      // ボタンで始める執筆支援
      for (const kind of WRITING_KINDS) {
        await runAndCompare(
          `支援（${kind}）`,
          OUTPUT[kind],
          () => yomi.startEntryAssist(ws.editorSession, ws.id, draft.id, kind),
          () => yomi.entryAssist(ws.editorSession, ws.id, draft.id, kind),
        );
      }
      // 代替テキストの案（画像ごと）
      await runAndCompare(
        "代替テキストの案",
        OUTPUT.altText,
        () => yomi.startAltTextSuggestion(ws.editorSession, ws.id, draft.id, draft.mediaId),
        () => yomi.altTextSuggestion(ws.editorSession, ws.id, draft.id, draft.mediaId),
      );
    } finally {
      await mockLlm.clearFaults(llmKey);
    }
  },
  900_000,
);

inv(
  "inv_bud_03",
  "編集者向けの支援の予算を超えて、支援のためのLLMは呼ばれない（執筆支援と代替テキストの案を含む）",
  async () => {
    const w = await world();
    const { yomi, llmKey } = w;
    const ws = await w.workspace("bud03", { assistCeiling: LARGE });
    await enableAssist(ws);
    const topic = wordMarker("bud03-topic");
    await publishNeighbor(ws, topic);

    /** 支援を1つ始める（代替テキストの案は画像ごと）。 */
    const start = (kind: (typeof WRITING_KINDS)[number] | "altText", d: { id: string; mediaId: string }) =>
      kind === "altText"
        ? yomi.startAltTextSuggestion(ws.editorSession, ws.id, d.id, d.mediaId)
        : yomi.startEntryAssist(ws.editorSession, ws.id, d.id, kind);
    const read = (kind: (typeof WRITING_KINDS)[number] | "altText", d: { id: string; mediaId: string }) =>
      kind === "altText"
        ? yomi.altTextSuggestion(ws.editorSession, ws.id, d.id, d.mediaId)
        : yomi.entryAssist(ws.editorSession, ws.id, d.id, kind);
    const ALL = [...WRITING_KINDS, "altText"] as const;

    // --- 1. 予算が尽きた後（月・日のそれぞれ）: どの支援も LLM を呼ばず、予算切れになる ---
    for (const period of ["month", "day"] as const) {
      const tiny = period === "month" ? { month: 1, day: null } : { month: null, day: 1 };
      await setBudget(ws, tiny, tiny);
      for (const kind of ALL) {
        await mockLlm.clearRequests(llmKey);
        await mockLlm.setStrategy(llmKey, { kind: "fixed", content: OUTPUT[kind] });
        const d = await draftWithImage(ws, topic, `bud03-${period}-${kind}`);
        await start(kind, d);
        const r = await settled(() => read(kind, d), `支援（${kind}）`);
        expect(r.state, `${period === "month" ? "月" : "日"}の予算が尽きたのに、支援（${kind}）の状態が予算切れでない`).toBe("budgetExhausted");
        expect(r.period, `支援（${kind}）の予算切れの期間が違う`).toBe(period);
        expect(chatRequests(await mockLlm.waitUntilQuiet(llmKey, 3_000)).length, `予算が尽きたのに、支援（${kind}）で LLM が呼ばれた`).toBe(0);
      }
    }

    // --- 2. 同時に何件も始めても、届いた入力・出力のトークン数の合計は上限を超えない（擬似LLMの数え方） ---
    /** 予算の範囲の数件だけが通る上限（1回の呼び出しより大きく、全部の合計よりずっと小さい）。 */
    const LIMIT = 3_000;
    const BURST = 12;
    for (const direction of ["input", "output"] as const) {
      const limited = { month: LIMIT, day: null };
      const open = { month: null, day: null };
      for (const kind of ALL) {
        // 予算は月ごとに数えるので、方向・機能ごとに新しいワークスペースで数える
        const fresh = await w.workspace(`bud03-${direction}-${kind}`, { assistCeiling: LARGE });
        await enableAssist(fresh);
        await setBudget(fresh, direction === "input" ? limited : open, direction === "output" ? limited : open);
        await mockLlm.clearRequests(llmKey);
        // 出力の上限を確かめるときは、1回の出力を長くして、上限に早く届くようにする
        // （どの支援でも1回あたり約 OUTPUT_CHARS 文字にし、BURST 件の合計が LIMIT を確実に超えるようにする）
        const OUTPUT_CHARS = 2_000;
        const content =
          direction === "output"
            ? Array.from({ length: Math.ceil(OUTPUT_CHARS / (OUTPUT[kind].length + 1)) }, () => OUTPUT[kind]).join("\n")
            : OUTPUT[kind];
        await mockLlm.setStrategy(llmKey, { kind: "fixed", content });
        const drafts = await Promise.all(
          Array.from({ length: BURST }, async (_, i) => {
            const id = (await yomi.createEntry(fresh.editorSession, fresh.id, {
              title: marker(`bud03-burst-${i}`),
              body: doc(
                heading("料金について"),
                paragraph(`${topic} の料金と提供時間を${TYPO}。` + "申し込みの手順は、窓口と画面のどちらからでも同じです。".repeat(30)),
                driver.body.image({ mediaId: (await yomi.uploadMedia(fresh.editorSession, fresh.id, { filename: "chart.png", bytes: pngImage(32, 24), contentType: "image/png" })).id }),
              ),
              fields: { summary: "まだ書いていない要約" },
            })).id;
            const draft = await yomi.getDraft(fresh.editorSession, fresh.id, id);
            const image = draft.body.content.find((n) => n.type === "image");
            return { id, mediaId: String(image?.attrs?.["mediaId"] ?? "") };
          }),
        );
        const startRaw = (d: { id: string; mediaId: string }) =>
          kind === "altText"
            ? yomi.startAltTextSuggestionRaw(asSession(fresh.editorSession), fresh.id, d.id, d.mediaId)
            : yomi.startEntryAssistRaw(asSession(fresh.editorSession), fresh.id, d.id, kind);
        await Promise.all(drafts.map((d) => startRaw(d)));
        const readFresh = (d: { id: string; mediaId: string }) =>
          kind === "altText"
            ? yomi.altTextSuggestion(fresh.editorSession, fresh.id, d.id, d.mediaId)
            : yomi.entryAssist(fresh.editorSession, fresh.id, d.id, kind);
        const results = await Promise.all(drafts.map((d) => settled(() => readFresh(d), `支援（${kind}）`)));
        const requests = chatRequests(await mockLlm.waitUntilQuiet(llmKey, 3_000));

        // 対照: 予算の範囲では呼ばれ、上限で止まった
        expect(requests.length, `予算が残っているのに、支援（${kind}）で LLM が呼ばれない`).toBeGreaterThan(0);
        expect(
          results.some((r) => r.state === "budgetExhausted"),
          `支援（${kind}）を ${BURST} 件始めても予算切れにならない（上限が小さすぎない舞台になっていない）`,
        ).toBe(true);

        if (direction === "input") {
          expect(mockUsageTokens(chatMessageTexts(requests)), `支援（${kind}）で、入力の予算を超えて LLM が呼ばれた（擬似LLMの数え方）`).toBeLessThanOrEqual(LIMIT);
        } else {
          expect(mockUsageTokens(requests.map(() => content)), `支援（${kind}）で、出力の予算を超えて LLM が呼ばれた（擬似LLMの数え方）`).toBeLessThanOrEqual(LIMIT);
        }
      }
    }
  },
  1_800_000,
);
