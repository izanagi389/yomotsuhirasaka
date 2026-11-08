-- ワークスペースの設定（管理者が変える。計画の10番）。yomi.workspaces は普段用のロールからは読むだけなので、別の表に持つ
-- （workspace_limits と同じ）。行がなければ既定の値。
-- take_over_policy: 編集の引き継ぎを誰ができるか（03-content-and-editing.md「同時編集（Q73）」）。
--   anyone: 下書きを編集できる人なら誰でも（既定）。permission: 重い権限「編集の引き継ぎ」を持つ人だけ（Q76）。
CREATE TABLE yomi.workspace_settings (
    workspace_id      uuid PRIMARY KEY REFERENCES yomi.workspaces (id) ON DELETE CASCADE,
    take_over_policy  text NOT NULL DEFAULT 'anyone' CHECK (take_over_policy IN ('anyone', 'permission'))
);
