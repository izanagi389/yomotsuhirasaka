-- キーワード検索のために、埋め込みのないチャンクも持つ（08-search-and-related.md、M3）。
-- - LLMの接続先がない・予算が尽きているときも、チャンク（本文）は作って置き、埋め込みは後から埋める。
-- - 埋め込みとそのモデル名・次元数は、3つとも空か、3つとも入っているかのどちらか。
ALTER TABLE search_chunks
    ALTER COLUMN model DROP NOT NULL,
    ALTER COLUMN dimensions DROP NOT NULL,
    ALTER COLUMN embedding DROP NOT NULL,
    ADD CONSTRAINT search_chunks_embedding_complete CHECK (
        (embedding IS NULL AND model IS NULL AND dimensions IS NULL)
        OR (embedding IS NOT NULL AND model IS NOT NULL AND dimensions IS NOT NULL)
    );
-- 埋め込みを埋める対象を探すため
CREATE INDEX search_chunks_unembedded ON search_chunks (entry_id) WHERE embedding IS NULL;
