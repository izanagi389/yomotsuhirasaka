-- ボタンを押したときだけ動く支援の結果（10-llm-and-budget.md「編集者向けのAI支援」、Q69, Q70、M4）。
-- - kind = 'stale'（見直しの手がかり。subject は「記事のID:下書きのリビジョン」）、'outline'（書くべき記事の構成案。
--   subject は質問ログの候補のID）。
-- - result は {hints: [{quote, reason}]} か {outline}（LLM の出力を CMS が検査したもの）。
-- - 24時間で消す。構成案は質問文から作るので、候補が消えたら（削除・保存期間・質問ログを無効に）一緒に消す。
CREATE TABLE assist_tasks (
    kind        text NOT NULL CHECK (kind IN ('stale', 'outline')),
    subject     text NOT NULL,
    state       text NOT NULL CHECK (state IN ('running', 'done', 'failed', 'disabled', 'budgetExhausted')),
    period      text CHECK (period IN ('month', 'day')),
    result      jsonb NOT NULL DEFAULT '{}',
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (kind, subject)
);
CREATE INDEX assist_tasks_created_at ON assist_tasks (created_at);
