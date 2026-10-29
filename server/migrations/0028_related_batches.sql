-- 関連記事の毎日の一括計算の記録（08-search-and-related.md「計算のタイミング」、M3）。ワークスペースに1行。
-- `last_run_on` は、最後に一括計算を積んだ予定の日（UTC）。今すぐの再計算（管理API）では書き換えない。
CREATE TABLE cms.related_batches (
    workspace_id  uuid PRIMARY KEY REFERENCES cms.workspaces (id) ON DELETE CASCADE,
    last_run_on   date NOT NULL
);
