-- 訪問者の検索語と質問文の埋め込みの使用量を、記事の埋め込みと別に数える（10-llm-and-budget.md「予算」）。
-- 上限は「埋め込みの作成」と同じ値を、それぞれに当てる。設定の列は増やさない。
ALTER TABLE yomi.budget_usage DROP CONSTRAINT budget_usage_kind_check;
ALTER TABLE yomi.budget_usage ADD CONSTRAINT budget_usage_kind_check
    CHECK (kind IN ('visitor_answers', 'editor_assist', 'embedding', 'visitor_embedding'));
