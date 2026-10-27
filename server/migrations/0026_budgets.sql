-- LLMの予算（10-llm-and-budget.md「予算」、Q70）。トークン数で数え、UTCの暦の月・日で区切る。
-- M3で使うのは「埋め込みの作成」の入力だけ（訪問者向けの回答・編集者向けの支援はM4）。

-- サーバー管理者が決める、ワークスペースの予算の天井（M3はインスタンスのキーなので、料金の天井はサーバー管理者が持つ）。
-- NULL なら設定ファイルの既定値（[llm] default_workspace_embedding_tokens_per_month / _per_day。なければ 0）。
ALTER TABLE cms.workspace_limits
    ADD COLUMN embedding_tokens_per_month bigint CHECK (embedding_tokens_per_month >= 0),
    ADD COLUMN embedding_tokens_per_day   bigint CHECK (embedding_tokens_per_day >= 0);

-- ワークスペースが決める予算の上限（重い権限 manageLlm）。効く上限は、使う瞬間に天井との小さいほう。
-- NULL は上限なし（天井がそのまま効く）。行がなければ両方 NULL と同じ。
CREATE TABLE cms.workspace_budgets (
    workspace_id      uuid NOT NULL REFERENCES cms.workspaces (id) ON DELETE CASCADE,
    kind              text NOT NULL CHECK (kind IN ('visitor_answers', 'editor_assist', 'embedding')),
    direction         text NOT NULL CHECK (direction IN ('input', 'output')),
    tokens_per_month  bigint CHECK (tokens_per_month >= 0),
    tokens_per_day    bigint CHECK (tokens_per_day >= 0),
    updated_by        uuid REFERENCES cms.users (id) ON DELETE SET NULL,
    updated_at        timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, kind, direction)
);

-- 期間ごとの使用量。呼ぶ前に多めの見積もりを足し（確保）、呼んだあと実際の数に直す（精算）。
-- 精算されなかった確保（呼んでいる途中で落ちたなど）は、使ったものとして残る。
-- exhausted_at: この期間で初めて予算が足りずに断った時刻（知らせは期間ごとに1回。上限を変えたら消す）。
CREATE TABLE cms.budget_usage (
    workspace_id  uuid NOT NULL REFERENCES cms.workspaces (id) ON DELETE CASCADE,
    kind          text NOT NULL CHECK (kind IN ('visitor_answers', 'editor_assist', 'embedding')),
    direction     text NOT NULL CHECK (direction IN ('input', 'output')),
    period        text NOT NULL CHECK (period IN ('month', 'day')),
    period_start  date NOT NULL,
    tokens        bigint NOT NULL DEFAULT 0 CHECK (tokens >= 0),
    exhausted_at  timestamptz,
    PRIMARY KEY (workspace_id, kind, direction, period, period_start)
);
