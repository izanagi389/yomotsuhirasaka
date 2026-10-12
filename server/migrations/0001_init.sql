-- 共通のスキーマ（cms）: インスタンス全体の情報を置く。
-- ワークスペースのロールからは触れない（INV-DB-04）。普段用のロールへの権限は migrate.rs の TABLE_PRIVILEGES で付ける。

-- ワークスペースの一覧。データの実体は、ワークスペースごとのスキーマ（db_schema）に置く。
CREATE TABLE cms.workspaces (
    id          uuid PRIMARY KEY,
    name        text NOT NULL,
    template    text NOT NULL,
    db_schema   text NOT NULL UNIQUE,
    db_role     text NOT NULL UNIQUE,
    created_at  timestamptz NOT NULL DEFAULT now()
);

-- 監査ログ（追記のみ）。普段用のロールには SELECT と INSERT だけを許す（INV-DB-03, INV-AUD-02）。
-- 記事の本文や質問文は書かない（INV-AUD-01）。IDと操作の種類だけを残す。
CREATE TABLE cms.audit_log (
    id            uuid PRIMARY KEY,
    workspace_id  uuid REFERENCES cms.workspaces (id),
    actor_user_id uuid,
    action        text NOT NULL,
    target_ids    text[] NOT NULL DEFAULT '{}',
    created_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX audit_log_workspace_created ON cms.audit_log (workspace_id, created_at);
