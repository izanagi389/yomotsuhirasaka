-- 編集者向けのAI支援の設定（10-llm-and-budget.md「編集者向けのAI支援」、Q69, Q177、M4）。ワークスペースに1行だけ。
-- 既定は無効（公開前の下書きが外部のLLMの提供会社に送られるため、重い権限 manageLlm を持つ人が明示的に有効にする）。
CREATE TABLE assist_settings (
    singleton  boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    enabled    boolean NOT NULL DEFAULT false
);
INSERT INTO assist_settings DEFAULT VALUES;
