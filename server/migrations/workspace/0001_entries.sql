-- ワークスペースのスキーマの土台: 記事と、公開した版（03-content-and-editing.md）。
-- search_path をワークスペースのスキーマにした中で実行する（名前は修飾しない）。
-- コンテンツタイプごとのテーブル（ct_<名前>、下書きのフィールドを1列ずつ持つ）は、テンプレートの移行で作る。

-- 記事（ロケールごとに1件。Q154）。
CREATE TABLE entries (
    id                    uuid PRIMARY KEY,
    content_type          text NOT NULL,
    locale                text NOT NULL,
    -- 同じ記事の翻訳どうしを結ぶID
    translation_group_id  uuid NOT NULL,
    -- 下書きのリビジョン番号（楽観ロック。公開版の「版」とは別物。Q137, Q149）
    draft_revision        bigint NOT NULL DEFAULT 1,
    -- 公開中の版（非公開なら NULL）
    published_version_id  uuid,
    created_by            uuid,
    created_at            timestamptz NOT NULL DEFAULT now(),
    updated_at            timestamptz NOT NULL DEFAULT now(),
    deleted_at            timestamptz
);
CREATE INDEX entries_type_locale ON entries (content_type, locale) WHERE deleted_at IS NULL;
CREATE INDEX entries_group ON entries (translation_group_id);

-- 公開した版。中身は公開した時点のフィールドのJSONのスナップショットで、後から書き換えない（本文の抹消を除く。Q134）。
CREATE TABLE versions (
    id              uuid PRIMARY KEY,
    entry_id        uuid NOT NULL REFERENCES entries (id),
    number          integer NOT NULL CHECK (number > 0),
    -- この版を作ったときのテンプレートの版
    schema_version  integer NOT NULL,
    -- 抹消したら NULL
    fields          jsonb,
    published_by    uuid,
    published_at    timestamptz NOT NULL DEFAULT now(),
    -- 「訂正として公開」したか
    correction      boolean NOT NULL DEFAULT false,
    erased_at       timestamptz,
    UNIQUE (entry_id, number)
);

ALTER TABLE entries
    ADD CONSTRAINT entries_published_version FOREIGN KEY (published_version_id) REFERENCES versions (id);
