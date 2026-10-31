-- 矛盾の検知の結果（10-llm-and-budget.md「編集者向けのAI支援」、Q69, Q70、M4）。記事の下書きのリビジョンごとに1行。
-- - 公開の確認画面が始め、非同期のジョブが埋める。公開の処理はこの表を見ない（INV-BUD-02）。
-- - conflicts は [{entryId, versionId, explanation}]（相手の記事と、そのときの公開版。説明は LLM の出力を CMS が検査したもの）。
--   読むときに相手の記事が今も見せてよいかを照合し、版の本文を抹消したら、その版を引いた行を消す。
-- - 24時間で消す（常駐の処理。読むときにも、それより古いものは使わない）。
CREATE TABLE assist_conflicts (
    entry_id    uuid NOT NULL REFERENCES entries (id) ON DELETE CASCADE,
    revision    bigint NOT NULL,
    state       text NOT NULL CHECK (state IN ('running', 'done', 'failed', 'disabled', 'budgetExhausted')),
    period      text CHECK (period IN ('month', 'day')),
    conflicts   jsonb NOT NULL DEFAULT '[]',
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (entry_id, revision)
);
CREATE INDEX assist_conflicts_created_at ON assist_conflicts (created_at);
