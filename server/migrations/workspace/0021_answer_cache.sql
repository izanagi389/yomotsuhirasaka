-- 回答キャッシュ（09-qa-chatbot.md「回答キャッシュ」、Q29, Q31, Q45, Q134、M4）。
-- - key は、正規化してマスクした質問文・ロケール・設定を、サーバーの鍵から導いた鍵で HMAC-SHA256 したもの。質問文の平文は持たない。
-- - sources は引用した出典（記事・版・ロケール・アンカー・パンくず）。URLと鮮度は持たず、返すときに今の状態から引く。
-- - 返す前に、出典が今も見せてよく、版が変わっていないかを照合する（合わなければ消して作り直す）。
CREATE TABLE answer_cache (
    key         bytea PRIMARY KEY,
    text        text NOT NULL,
    truncated   boolean NOT NULL,
    sources     jsonb NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    expires_at  timestamptz NOT NULL
);
CREATE INDEX answer_cache_created ON answer_cache (created_at);
CREATE INDEX answer_cache_sources ON answer_cache USING gin (sources jsonb_path_ops);
