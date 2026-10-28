// サイト内検索の約束: INV-SRC-01〜05（M3、08-search-and-related.md）
//
// - 検索は配信APIの GraphQL（Persisted Queries）で行う。舞台は、本文（body）を検索の対象にしたテンプレートの
//   2つのワークスペース（support/world.ts の lazySearchWorld）。テスト環境のCMSは擬似LLMで埋め込みを作る。
// - 埋め込みは公開の後で非同期に作られるので、「検索に出るようになる」まで待ってから確かめる（対照）。
// - ベクトル検索は類似度の足切りをしないので、ワークスペースの公開記事は、どの検索語でも結果に出うる。
//   そのため「出ない」ことは、検索語によらず、結果のどこにも出ないことで確かめる。
// - テスト環境のPostgreSQLには、日本語のキーワード検索の拡張（pg_bigm・PGroonga）を入れていない（setup.sh、CI）。
//   INV-SRC-05 は、この環境で確かめる。キーワード検索が動くことは、拡張の入った環境でサーバーの結合テストが確かめる。

import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asSession, type RegisteredQuery, type SearchMode, type SearchResult } from "./support/driver.js";
import { expectSuccess } from "./support/assert.js";
import { simpleBody } from "./support/body.js";
import { lazySearchWorld, marker, sleep, type SearchWorkspace } from "./support/world.js";

const world = lazySearchWorld();

/** 共通の本文（類似度を比べるテストで、検索の対象の内容をそろえるため）。 */
const BASE_TEXT = "サイト内検索の約束を確かめるための本文です。公開した記事だけが、検索の結果に出ます。";

async function publish(ws: SearchWorkspace, body: string, options: { title?: string; fields?: Record<string, unknown> } = {}): Promise<string> {
  const { cms } = await world();
  const { id } = await cms.createEntry(ws.editorSession, ws.id, { title: options.title ?? marker("src-title"), body: simpleBody(body), fields: options.fields });
  await cms.publishEntry(ws.editorSession, ws.id, id);
  return id;
}

async function search(ws: SearchWorkspace, text: string): Promise<SearchResult> {
  const { cms } = await world();
  return cms.search(ws.id, ws.readKey.secret, ws.searchQuery, { text, first: 50 });
}

const idsOf = (r: SearchResult): string[] => r.hits.map((h) => h.entryId);

/** 記事が検索に出るようになる（埋め込みができる）まで待つ。 */
async function waitSearchable(ws: SearchWorkspace, text: string, entryIds: string[], timeoutMs = 120_000): Promise<SearchResult> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const r = await search(ws, text);
    expect(r.vectorSearch, "ベクトル検索が使えない（擬似LLMにつながっていない、または予算がない）").toBe("available");
    if (entryIds.every((id) => idsOf(r).includes(id))) return r;
    if (Date.now() > deadline) throw new Error(`公開した記事が ${timeoutMs}ms たっても検索に出ない: ${entryIds.filter((id) => !idsOf(r).includes(id)).join(", ")}`);
    await sleep(1);
  }
}

inv(
  "inv_src_01",
  "非公開・削除・チャットボットの対象外にした記事は、検索用データの物理削除が終わる前でも、検索結果に含まれない",
  async () => {
    const { cms, a } = await world();
    const word = marker("src01");
    const kept = await publish(a, `${word} ${BASE_TEXT}`);
    const unpublished = await publish(a, `${word} ${BASE_TEXT} 非公開にする記事`);
    const deleted = await publish(a, `${word} ${BASE_TEXT} 削除する記事`);
    const switchedOff = await publish(a, `${word} ${BASE_TEXT} 対象外にする記事`);
    // 対照: 4件とも検索に出る
    await waitSearchable(a, word, [kept, unpublished, deleted, switchedOff]);

    await cms.unpublishEntry(a.editorSession, a.id, unpublished);
    await cms.deleteEntry(a.editorSession, a.id, deleted);
    await cms.reauthenticate(a.adminSession, a.admin);
    expectSuccess(await cms.setPublishSettingsRaw(asSession(a.adminSession), a.id, switchedOff, { useForChatbot: false }), "チャットボットの対象外にする");

    // すぐに（物理削除のジョブが終わる前に）検索しても出ない。少し間をおいても出ない
    for (const wait of [0, 3]) {
      await sleep(wait);
      const r = await search(a, word);
      expect(idsOf(r), "非公開にした記事が検索結果に含まれる").not.toContain(unpublished);
      expect(idsOf(r), "削除した記事が検索結果に含まれる").not.toContain(deleted);
      expect(idsOf(r), "チャットボットの対象外にした記事が検索結果に含まれる").not.toContain(switchedOff);
      // 対照: 公開のままの記事は出る
      expect(idsOf(r), "公開のままの記事が検索結果に出ない").toContain(kept);
    }
  },
  240_000,
);

inv(
  "inv_src_02",
  "予約公開の記事は、公開時刻より前には検索結果に含まれない",
  async () => {
    const { cms, a } = await world();
    const word = marker("src02");
    const newWord = marker("src02-new");
    const live = await publish(a, `${word} ${BASE_TEXT}`);
    // まだ公開していない記事を予約する
    const { id: scheduled } = await cms.createEntry(a.editorSession, a.id, { title: marker("src02-title"), body: simpleBody(`${word} ${BASE_TEXT} 予約した記事`) });
    // 公開中の記事の、新しい内容の版を予約する（公開中の古い版は出てよいが、新しい内容は出ない）
    const updated = await publish(a, `${word} ${BASE_TEXT} 更新を予約する記事`);
    const at = new Date(Date.now() + 75_000);
    await cms.schedulePublish(a.editorSession, a.id, scheduled, at);
    await cms.saveDraft(a.editorSession, a.id, updated, { body: simpleBody(`${newWord} ${BASE_TEXT} 予約した新しい内容`) });
    await cms.schedulePublish(a.editorSession, a.id, updated, at);
    await waitSearchable(a, word, [live, updated]);

    // 公開時刻の少し前まで、何度確かめても出ない
    while (Date.now() < at.getTime() - 10_000) {
      for (const text of [word, newWord]) {
        const r = await search(a, text);
        expect(idsOf(r), "予約した記事が、公開時刻より前に検索結果に含まれる").not.toContain(scheduled);
        expect(JSON.stringify(r), "予約した新しい版の内容が、公開時刻より前に検索結果に含まれる").not.toContain(newWord);
        // 対照: 公開中の記事は出る
        expect(idsOf(r), "公開中の記事が検索結果に出ない").toContain(live);
      }
      await sleep(5);
    }

    // 対照: 公開時刻を過ぎれば出る（予約が効いていない、のではない）
    const after = await waitSearchable(a, newWord, [scheduled, updated], 150_000);
    expect(after.hits.find((h) => h.entryId === updated)?.excerpt, "予約した新しい版の内容が、公開の後も検索に出ない").toContain(newWord);
  },
  360_000,
);

inv(
  "inv_src_03",
  "スキーマで検索・質問応答の対象として許可していないフィールドの内容は、記事側のスイッチの状態にかかわらず、検索結果にも回答の根拠にも含まれない",
  async () => {
    const { cms, a } = await world();
    // テンプレートで対象にしているのは本文（body）だけ。副題（subtitle）は対象外
    const word = marker("src03");
    const title = `類似度を比べる記事 ${marker("src03-title")}`;
    // 対象外のフィールドにだけ目印を入れた記事と、それ以外が同じ記事
    const outside = await publish(a, BASE_TEXT, { title, fields: { subtitle: word } });
    const same = await publish(a, BASE_TEXT, { title });
    // 対照: 対象のフィールドに目印を入れた記事
    const inside = await publish(a, `${word} ${BASE_TEXT}`);
    // 記事側のスイッチ（チャットボットに使う）を明示して ON にしても、対象外のフィールドは使われない
    await cms.reauthenticate(a.adminSession, a.admin);
    expectSuccess(await cms.setPublishSettingsRaw(asSession(a.adminSession), a.id, outside, { useForChatbot: true }), "チャットボットに使う");

    const r = await waitSearchable(a, word, [outside, same, inside]);
    // 対象外のフィールドの内容は、結果のどこにも出ない
    expect(JSON.stringify(r.hits.filter((h) => h.entryId !== inside)), "対象外のフィールドの内容が検索結果に含まれる").not.toContain(word);
    // 対象外のフィールドの内容は、一致の度合いにも効かない（それ以外が同じ記事と、類似度が同じ）
    const score = (id: string) => r.hits.find((h) => h.entryId === id)!.score;
    expect(Math.abs(score(outside) - score(same)), "対象外のフィールドの内容が、検索の一致に使われている").toBeLessThan(1e-6);
    // 対照: 対象のフィールドの内容は、抜粋に出て、一致の度合いも高い
    expect(r.hits.find((h) => h.entryId === inside)!.excerpt, "対象のフィールドの内容が抜粋に出ない").toContain(word);
    expect(score(inside), "対象のフィールドの内容が、検索の一致に使われていない").toBeGreaterThan(score(same));
  },
  240_000,
);

inv(
  "inv_src_04",
  "検索結果には、検索したワークスペース以外の記事が含まれない",
  async () => {
    const { a, b } = await world();
    const word = marker("src04");
    const onlyB = marker("src04-b");
    const inA = await publish(a, `${word} ${BASE_TEXT}`);
    const inB = await publish(b, `${word} ${onlyB} ${BASE_TEXT}`);
    // 対照: どちらのワークスペースでも、自分の記事は検索に出る
    await waitSearchable(a, word, [inA]);
    await waitSearchable(b, onlyB, [inB]);

    for (const text of [word, onlyB]) {
      const r = await search(a, text);
      expect(idsOf(r), "Aの検索結果に、Bの記事が含まれる").not.toContain(inB);
      expect(JSON.stringify(r), "Aの検索結果に、Bの記事の内容が含まれる").not.toContain(onlyB);
      expect(idsOf(r), "Aの検索結果に、Aの記事が出ない").toContain(inA);
    }
  },
  240_000,
);

inv(
  "inv_src_05",
  "日本語キーワード検索の拡張がない環境でも、ベクトル検索は動作し、キーワード検索だけが無効であることが応答で示される",
  async () => {
    const { cms, a } = await world();
    const word = marker("src05");
    const entry = await publish(a, `${word} ${BASE_TEXT}`);
    // 検索の方法（mode）を指定するクエリ
    const withMode: RegisteredQuery = await cms.registerQuery(a.id, a.registerKey.secret, { kind: "searchWithMode" });
    const searchBy = (mode: SearchMode): Promise<SearchResult> => cms.search(a.id, a.readKey.secret, withMode, { text: word, first: 50, mode });

    // 方法を指定しない（既定の、2つを混ぜる検索）: ベクトル検索の結果が返り、キーワード検索だけが無効と示される
    const byDefault = await waitSearchable(a, word, [entry]);
    expect(byDefault.keywordSearch, "既定の検索で、キーワード検索が無効と示されない").toBe("disabled");

    // 2つを混ぜる検索を指定しても同じ（エラーにならず、ベクトル検索の結果が返る）
    const hybrid = await searchBy("hybrid");
    expect(hybrid.vectorSearch, "2つを混ぜる検索で、ベクトル検索が使えない").toBe("available");
    expect(hybrid.keywordSearch, "2つを混ぜる検索で、キーワード検索が無効と示されない").toBe("disabled");
    expect(idsOf(hybrid), "2つを混ぜる検索で、公開した記事が結果に出ない").toContain(entry);

    // ベクトル検索だけを指定: 動作する
    const vector = await searchBy("vector");
    expect(vector.vectorSearch, "ベクトル検索だけを指定したとき、ベクトル検索が使えない").toBe("available");
    expect(vector.keywordSearch, "ベクトル検索だけを指定したとき、キーワード検索が無効と示されない").toBe("disabled");
    expect(idsOf(vector), "ベクトル検索だけを指定したとき、公開した記事が結果に出ない").toContain(entry);

    // キーワード検索だけを指定: エラーにはならず、結果は空で、キーワード検索が無効であることが示される
    const keyword = await searchBy("keyword");
    expect(keyword.keywordSearch, "キーワード検索だけを指定したとき、キーワード検索が無効と示されない").toBe("disabled");
    expect(keyword.hits, "拡張がないのに、キーワード検索の結果が返る").toEqual([]);
  },
  240_000,
);
