-- 埋め込みモデルの切り替えの開始（08-search-and-related.md「モデルの変更」、Q30・Q138）。インスタンスに1行。
-- 設定に切り替え先のモデル（[llm.next_embedding]）を書いただけでは、埋め込みを作らない。サーバー管理者が見積もりを見て
-- 開始すると、ここにそのモデルを記録する。ジョブは、ここのモデルが設定の切り替え先と同じときだけ、新しい埋め込みを作る。
CREATE TABLE yomi.embedding_switch (
    id          boolean PRIMARY KEY DEFAULT true CHECK (id),
    model       text NOT NULL,
    dimensions  integer NOT NULL,
    started_by  uuid REFERENCES yomi.users (id) ON DELETE SET NULL,
    started_at  timestamptz NOT NULL DEFAULT now()
);
