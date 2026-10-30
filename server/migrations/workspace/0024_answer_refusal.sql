-- 拒否する語（09-qa-chatbot.md「拒否する語」、2026-10-30、M4）。回答の設定（answer_settings）の列として持つ。
-- - refuse_words: 拒否する語（100件まで。1件100文字まで。照合は正規化してからの部分一致）
-- - refuse_credit_card・refuse_my_number: 組み込みの種類（既定は OFF）
ALTER TABLE answer_settings
    ADD COLUMN refuse_words        text[] NOT NULL DEFAULT '{}' CHECK (cardinality(refuse_words) <= 100),
    ADD COLUMN refuse_credit_card  boolean NOT NULL DEFAULT false,
    ADD COLUMN refuse_my_number    boolean NOT NULL DEFAULT false;
