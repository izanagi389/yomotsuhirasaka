-- 執筆支援（10-llm-and-budget.md「編集者向けのAI支援」の4〜7、2026-11-17）の結果も、ボタンを押したときだけ動く支援の
-- 結果の表に残す。
-- - kind = 'structure'（構成の確認。subject は「記事のID:下書きのリビジョン」、答えられなかった質問を含める回は末尾に「:q」）。
--   result は {findings: [{kind, text, entryId?, versionId?, questionKey?}]}（質問文は残さず、候補のキーだけ）。
-- - 'proofread'・'summary'・'title'・'alt_text' は、続く執筆支援（校正・要約とタイトルの案・代替テキストの案）のために、
--   ここでまとめて認めておく。
ALTER TABLE assist_tasks DROP CONSTRAINT assist_tasks_kind_check;
ALTER TABLE assist_tasks ADD CONSTRAINT assist_tasks_kind_check
    CHECK (kind IN ('stale', 'outline', 'structure', 'proofread', 'summary', 'title', 'alt_text'));
