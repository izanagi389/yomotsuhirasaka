-- サイトのプレビュー用ルートのパス（計画の13番）。管理画面は「サイトのオリジン ＋ このパス ＋ ?token=…」を開く。
-- 既定は公式のNuxtモジュールの既定（/_cms/preview）。
ALTER TABLE cms.workspace_settings ADD COLUMN preview_path text NOT NULL DEFAULT '/_cms/preview';
