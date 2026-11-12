-- ワークスペースの紋（2026-11-12、持ち主の判断）。管理画面で、ワークスペースを見分けるための飾り。
-- 形は12種類、色は6色から、ワークスペースの管理者が選ぶ。選んでいなければ（NULL）管理画面がIDから決める。
-- 普段用のロールが書ける workspace_settings に置く（yomi.workspaces の権限は変えない）。
ALTER TABLE yomi.workspace_settings
  ADD COLUMN crest_motif text,
  ADD COLUMN crest_color text,
  ADD CONSTRAINT workspace_settings_crest_motif_check CHECK (crest_motif IN
    ('shippo', 'asanoha', 'hishi', 'sakura', 'ume', 'kikyo', 'fuji', 'take', 'nami', 'kumo', 'ogi', 'kuyo')),
  ADD CONSTRAINT workspace_settings_crest_color_check CHECK (crest_color IN
    ('ai', 'asagi', 'shu', 'yamabuki', 'seiji', 'nezumi')),
  ADD CONSTRAINT workspace_settings_crest_pair_check CHECK ((crest_motif IS NULL) = (crest_color IS NULL));
