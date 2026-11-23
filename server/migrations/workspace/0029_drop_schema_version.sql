-- 公開した版と予約に、公開したときのテンプレートの版を記録しない（02-schema-and-delivery-api.md「移行」、
-- 2026-11-23 の持ち主の判断。テンプレートの版をやめたため）。
ALTER TABLE versions DROP COLUMN schema_version;
ALTER TABLE schedules DROP COLUMN schema_version;
