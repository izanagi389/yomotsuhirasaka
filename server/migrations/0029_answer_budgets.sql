-- 訪問者向けの回答の予算の天井（10-llm-and-budget.md「予算」、Q21, Q70、M4）。入力・出力別に、UTCの暦の月・日で数える。
-- NULL なら設定ファイルの既定値（[llm] default_workspace_answer_{input,output}_tokens_per_{month,day}。なければ 0）。
-- 使用量とワークスペースの上限は、埋め込みと同じ budget_usage・workspace_budgets（kind = 'visitor_answers'）に持つ。
ALTER TABLE yomi.workspace_limits
    ADD COLUMN answer_input_tokens_per_month  bigint CHECK (answer_input_tokens_per_month >= 0),
    ADD COLUMN answer_input_tokens_per_day    bigint CHECK (answer_input_tokens_per_day >= 0),
    ADD COLUMN answer_output_tokens_per_month bigint CHECK (answer_output_tokens_per_month >= 0),
    ADD COLUMN answer_output_tokens_per_day   bigint CHECK (answer_output_tokens_per_day >= 0);
