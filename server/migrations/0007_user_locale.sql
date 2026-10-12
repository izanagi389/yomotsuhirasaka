-- 利用者ごとの言語（Q198）。NULL ならインスタンスの既定の言語を使う。
ALTER TABLE cms.users
    ADD COLUMN locale text CHECK (locale IN ('en', 'ja'));
