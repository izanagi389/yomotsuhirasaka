-- ワークスペースへの所属とロール（Q76, Q101, Q121）。
-- 利用者はインスタンス単位で存在し、複数のワークスペースに所属できる。
-- 権限はリクエストのたびにこの表と照合し、セッションには保存しない。

CREATE TABLE yomi.memberships (
    workspace_id  uuid NOT NULL REFERENCES yomi.workspaces (id) ON DELETE CASCADE,
    user_id       uuid NOT NULL REFERENCES yomi.users (id) ON DELETE CASCADE,
    role          text NOT NULL CHECK (role IN ('admin', 'editor', 'author', 'viewer')),
    created_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, user_id)
);
CREATE INDEX memberships_user ON yomi.memberships (user_id);
