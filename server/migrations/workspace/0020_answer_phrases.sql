-- 回答APIの訪問者向けの定型文（09-qa-chatbot.md、03-content-and-editing.md「訪問者向けの文言」、M4）。
-- コンテンツのロケールごとに1行。ワークスペースで編集できる（編集の画面は語り口の作業で作る）。
-- ロケールの行がなければ英語の行を使う。
CREATE TABLE answer_phrases (
    locale          text PRIMARY KEY,
    -- 「該当する情報がありません」（回答に使える記事がない、引用のない回答、答えられないという宣言など）
    no_information  text NOT NULL
);
INSERT INTO answer_phrases (locale, no_information) VALUES
    ('ja', '該当する情報がありません。'),
    ('en', 'No relevant information was found.');
