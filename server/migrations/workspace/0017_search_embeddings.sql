-- 埋め込みを、チャンクとは別の表に持つ（08-search-and-related.md「モデルの変更」、M3）。
-- - 埋め込みは、テキスト（content_hash）とモデルと次元数ごとに1つ。同じテキストのチャンクは、記事をまたいでも使い回す（Q58）。
-- - モデルの切り替えの間は、旧モデルと新しいモデルの埋め込みを並べて持つ（Q30）。
-- - チャンクは本文だけを持つ。検索のときに、テキストの要約とモデルで結び付ける。
-- - どのチャンクにも使われなくなった埋め込みは、検索用データのジョブが後から消す。
CREATE TABLE search_embeddings (
    content_hash  text NOT NULL,
    model         text NOT NULL,
    dimensions    integer NOT NULL,
    embedding     real[] NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (content_hash, model, dimensions)
);

-- 今のチャンクにある埋め込みを移す
INSERT INTO search_embeddings (content_hash, model, dimensions, embedding, created_at)
SELECT DISTINCT ON (content_hash, model, dimensions) content_hash, model, dimensions, embedding, created_at
FROM search_chunks
WHERE embedding IS NOT NULL
ORDER BY content_hash, model, dimensions, created_at DESC;

-- チャンクからは、埋め込みとそのモデルの列を外す（その列を使う索引も一緒になくなる）
ALTER TABLE search_chunks
    DROP CONSTRAINT search_chunks_embedding_complete,
    DROP COLUMN embedding,
    DROP COLUMN model,
    DROP COLUMN dimensions;
DROP INDEX IF EXISTS search_chunks_unembedded;
DROP INDEX IF EXISTS search_chunks_hash;
DROP INDEX IF EXISTS search_chunks_locale;
CREATE INDEX search_chunks_hash ON search_chunks (content_hash);
CREATE INDEX search_chunks_locale ON search_chunks (locale);
