-- ワークスペースの埋め込みの上書きと、ワークスペースごとの埋め込みのモデルの切り替え（10-llm-and-budget.md「接続設定の単位」、
-- 08-search-and-related.md「モデルの変更」、M4）。
-- - workspace_llm.embedding_model・embedding_dimensions: ワークスペースが選んだ埋め込みのモデル（サーバーの設定の
--   [[llm.allowed_embedding_models]] の中から）。なければインスタンスの設定のモデル。
-- - workspace_embedding_switch: ワークスペースの manageLlm を持つ人が開始した切り替え先（見積もりを見てから）。
--   新しいモデルの埋め込みがそろうまで、前のモデルで検索を続ける。次元数が合わなければ failed_code を記録して止める。
ALTER TABLE cms.workspace_llm
    ADD COLUMN embedding_model       text,
    ADD COLUMN embedding_dimensions  integer CHECK (embedding_dimensions BETWEEN 1 AND 2000),
    ADD CONSTRAINT workspace_llm_embedding_pair CHECK ((embedding_model IS NULL) = (embedding_dimensions IS NULL));

CREATE TABLE cms.workspace_embedding_switch (
    workspace_id  uuid PRIMARY KEY REFERENCES cms.workspaces (id) ON DELETE CASCADE,
    model         text NOT NULL,
    dimensions    integer NOT NULL CHECK (dimensions BETWEEN 1 AND 2000),
    started_by    uuid REFERENCES cms.users (id) ON DELETE SET NULL,
    started_at    timestamptz NOT NULL DEFAULT now(),
    failed_code   text
);
