-- ワークスペースの LLM の接続設定の上書きに、画像を読むモデルを足す（代替テキストの案。2026-11-17、
-- 10-llm-and-budget.md「編集者向けのAI支援」の7）。接続先とキーはチャットと同じ（上書きの接続先は許可の一覧の名前）。
ALTER TABLE yomi.workspace_llm ADD COLUMN vision_model text;
