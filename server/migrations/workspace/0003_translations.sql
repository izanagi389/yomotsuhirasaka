-- 翻訳の作成と、元の記事・版の記録（03-content-and-editing.md「下書き・公開・版」、Q154, Q156, Q159）。

-- 元の記事（翻訳の元にした、同じ翻訳グループの記事）。元なしなら NULL。
-- 元の記事が削除されても値は残し、読むときに「元なし」として扱う（記録を後から失わないため）。
ALTER TABLE entries
    ADD COLUMN source_entry_id uuid REFERENCES entries (id),
    -- 反映済みとして記録した、元の記事の版（作成の時点で元の記事が公開されていなければ NULL）
    ADD COLUMN reflected_source_version_id uuid REFERENCES versions (id);

-- 翻訳グループの中で、1つのロケールの記事は1つだけ
CREATE UNIQUE INDEX entries_group_locale ON entries (translation_group_id, locale) WHERE deleted_at IS NULL;
