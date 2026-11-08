-- ワークスペースごとの上限（13-security.md「メディア」、Q96, Q100）。
-- workspaces の権限を広げないため、別のテーブルにする。行がなければ設定の既定値を使う。
CREATE TABLE yomi.workspace_limits (
    workspace_id  uuid PRIMARY KEY REFERENCES yomi.workspaces (id) ON DELETE CASCADE,
    -- メディアの容量の上限（バイト）。NULL なら設定の既定値
    media_bytes   bigint CHECK (media_bytes >= 0),
    updated_by    uuid REFERENCES yomi.users (id) ON DELETE SET NULL,
    updated_at    timestamptz NOT NULL DEFAULT now()
);
