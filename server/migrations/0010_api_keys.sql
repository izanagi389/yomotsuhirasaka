-- 配信APIのキー（Q13, Q93, Q94, Q153）。ワークスペースごと。
-- 共通のスキーマに置き、ワークスペースのロールからは触れない（13-security.md）。
-- 発行時に一度だけ表示し、DBにはハッシュだけを保存する（INV-KEY-01）。
CREATE TABLE cms.api_keys (
    id             uuid PRIMARY KEY,
    workspace_id   uuid NOT NULL REFERENCES cms.workspaces (id) ON DELETE CASCADE,
    name           text NOT NULL,
    key_hash       bytea NOT NULL UNIQUE,
    display_prefix text NOT NULL,
    -- read: 公開版の読み取り、register: クエリの登録
    scopes         text[] NOT NULL CHECK (scopes <@ ARRAY['read', 'register']::text[] AND cardinality(scopes) > 0),
    -- 用途（回数制限の既定値だけに影響し、権限は変えない。Q153）
    purpose        text NOT NULL CHECK (purpose IN ('build', 'server')),
    created_by     uuid REFERENCES cms.users (id) ON DELETE SET NULL,
    created_at     timestamptz NOT NULL DEFAULT now(),
    -- NULL なら期限なし
    expires_at     timestamptz,
    last_used_at   timestamptz,
    revoked_at     timestamptz
);
CREATE INDEX api_keys_workspace ON cms.api_keys (workspace_id) WHERE revoked_at IS NULL;
