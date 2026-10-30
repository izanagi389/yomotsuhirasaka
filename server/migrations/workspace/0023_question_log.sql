-- 質問ログ（09-qa-chatbot.md「質問ログ」、Q14, Q20、M4）。
-- - 設定（1行だけ）: 有効か（既定は無効。ワークスペースごとのオプトイン）と保存期間（日。7〜365、既定は90）。
-- - question_log: 答えられなかった質問だけ。マスクした質問文・ロケール・時刻。key は正規化した質問文のハッシュ
--   （同じ質問をまとめて「書くべき記事の候補」にする）。訪問者の識別子やIPアドレスは持たない。保存期間を過ぎたら消す。
-- - question_counts: 日（UTC）と結果ごとの件数だけ（答えた・情報なし・拒否・エラー）。質問文は持たない。
CREATE TABLE question_log_settings (
    singleton       boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    enabled         boolean NOT NULL DEFAULT false,
    retention_days  integer NOT NULL DEFAULT 90 CHECK (retention_days BETWEEN 7 AND 365)
);
INSERT INTO question_log_settings DEFAULT VALUES;

CREATE TABLE question_log (
    id        uuid PRIMARY KEY,
    key       text NOT NULL,
    question  text NOT NULL,
    locale    text NOT NULL,
    asked_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX question_log_asked ON question_log (asked_at);
CREATE INDEX question_log_key ON question_log (key);

CREATE TABLE question_counts (
    day      date NOT NULL,
    outcome  text NOT NULL CHECK (outcome IN ('answered', 'no_information', 'refused', 'error')),
    count    bigint NOT NULL,
    PRIMARY KEY (day, outcome)
);
