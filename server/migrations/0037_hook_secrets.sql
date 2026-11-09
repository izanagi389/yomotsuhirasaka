-- フックが読む秘密（04-hooks.md「秘密」、第三者の確認の R41）。ワークスペースごとに持ち、値は暗号化する。
CREATE TABLE yomi.hook_secrets (
    workspace_id uuid NOT NULL REFERENCES yomi.workspaces (id) ON DELETE CASCADE,
    name         text NOT NULL,
    value_enc    bytea NOT NULL,
    updated_by   uuid REFERENCES yomi.users (id) ON DELETE SET NULL,
    updated_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, name)
);
