-- ワークスペースが今どのモデルで検索しているか（08-search-and-related.md「モデルの変更」、Q30）。1行。
-- - 行がなければ、設定の今のモデル（[llm] embedding_model）を使う。
-- - 切り替え先のモデルの埋め込みがすべてのチャンクにそろったら、ジョブが active を切り替え、それまでのモデルを previous に残す。
--   切り替えから30日は previous の埋め込みを残し、切り戻せるようにする。
-- - paused_at: 予算が尽きて埋め込みの作成が止まった時刻（進んだら消す）。
CREATE TABLE search_models (
    id                   boolean PRIMARY KEY DEFAULT true CHECK (id),
    active_model         text NOT NULL,
    active_dimensions    integer NOT NULL,
    previous_model       text,
    previous_dimensions  integer,
    switched_at          timestamptz,
    paused_at            timestamptz
);
