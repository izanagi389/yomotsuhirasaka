-- ワークスペースの削除（2026-11-12、INV-TEN-07・08、INV-DB-05）。
-- 監査ログの行は、ワークスペースを削除しても通常の保存期間まで残す（ワークスペースのIDはそのまま持つ）。
-- そのため、監査ログからワークスペースへの外部キーを外す。
ALTER TABLE yomi.audit_log DROP CONSTRAINT audit_log_workspace_id_fkey;
