-- 編集者向けの支援の予算の天井（10-llm-and-budget.md「予算」、Q70、M4）。入力・出力別に、UTCの暦の月・日で数える。
-- NULL なら設定ファイルの既定値（[llm] default_workspace_assist_{input,output}_tokens_per_{month,day}。なければ 0）。
-- 使用量とワークスペースの上限は、ほかの予算と同じ budget_usage・workspace_budgets（kind = 'editor_assist'）に持つ。
ALTER TABLE yomi.workspace_limits
    ADD COLUMN assist_input_tokens_per_month  bigint CHECK (assist_input_tokens_per_month >= 0),
    ADD COLUMN assist_input_tokens_per_day    bigint CHECK (assist_input_tokens_per_day >= 0),
    ADD COLUMN assist_output_tokens_per_month bigint CHECK (assist_output_tokens_per_month >= 0),
    ADD COLUMN assist_output_tokens_per_day   bigint CHECK (assist_output_tokens_per_day >= 0);
