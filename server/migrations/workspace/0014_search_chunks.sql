-- 検索用データ（08-search-and-related.md、M3）。公開中の版の、検索の対象のフィールドから作ったチャンクと、その埋め込み。
-- - 検索のたびに、記事の今の公開状態と記事ごとのスイッチを照合する（ここにあっても、見せてよいとは限らない。原則3、INV-SRC-01）。
--   対象でなくなった記事の行は、ジョブが後から消す（物理削除）。
-- - 埋め込みは real[] で持ち、検索のときに pgvector の vector に変換して比べる（拡張のない環境でも、ワークスペースを作れるように）。
-- - 同じテキスト（content_hash）と同じモデルの埋め込みは、作り直さずに使い回す（Q58）。
CREATE TABLE search_chunks (
    id            uuid PRIMARY KEY,
    entry_id      uuid NOT NULL REFERENCES entries (id),
    version_id    uuid NOT NULL REFERENCES versions (id),
    locale        text NOT NULL,
    -- 記事の中での順番
    seq           integer NOT NULL,
    -- セクションのアンカーID（見出しがなければ NULL）
    anchor        text,
    -- 「タイトル ＞ 大見出し ＞ 小見出し」（タイトルは、タイトルのフィールドが検索の対象のときだけ）
    breadcrumb    text[] NOT NULL,
    -- 本文（変換器の Markdown。見出しの行は見出しの文字だけ）
    content       text NOT NULL,
    -- 埋め込みに渡した文字列（パンくずと本文）の SHA-256
    content_hash  text NOT NULL,
    -- 埋め込みのモデル名と次元数（モデルの変更に備えて、一緒に残す。Q30）
    model         text NOT NULL,
    dimensions    integer NOT NULL,
    embedding     real[] NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    UNIQUE (entry_id, seq)
);
CREATE INDEX search_chunks_hash ON search_chunks (content_hash, model);
CREATE INDEX search_chunks_locale ON search_chunks (locale, model);

-- 検索用データを作った「決め方」（対象のフィールド・チャンクの決まり・変換器の版・モデル）の要約。
-- 変わったら（テンプレートのデプロイ、変換器やモデルの変更）、すべての記事のチャンクを見直す（テキストが同じものの埋め込みは使い回す）。
CREATE TABLE search_state (
    id        boolean PRIMARY KEY DEFAULT true CHECK (id),
    plan_key  text NOT NULL
);
