-- 回答の語り口（09-qa-chatbot.md「語り口」、Q24, Q25、M4）。ワークスペースに1行だけ。
-- - politeness・length は構造化された設定（プロンプトでの指示だけ。出力の上限や打ち切りの長さは変えない）。
-- - 自由記述のシステムプロンプトは、使うかどうかのスイッチ（既定は使わない）と、文を別に持つ。使わない設定でも文は残す。
-- 訪問者向けの「情報なし」の文は answer_phrases（0020）。
CREATE TABLE answer_settings (
    singleton            boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    politeness           text NOT NULL DEFAULT 'polite' CHECK (politeness IN ('polite', 'casual')),
    length               text NOT NULL DEFAULT 'standard' CHECK (length IN ('short', 'standard', 'detailed')),
    free_prompt_enabled  boolean NOT NULL DEFAULT false,
    free_prompt          text NOT NULL DEFAULT '' CHECK (char_length(free_prompt) <= 2000)
);
INSERT INTO answer_settings DEFAULT VALUES;
