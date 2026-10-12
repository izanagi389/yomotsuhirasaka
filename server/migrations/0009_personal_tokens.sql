-- CLI用の個人トークン（Q91, Q119）。
-- 有効な権限は、トークンの範囲とリクエストの時点での本人のロールの重なりだけ。
-- 発行時に一度だけ表示し、DBにはハッシュだけを保存する（INV-KEY-01）。
CREATE TABLE cms.personal_tokens (
    id             uuid PRIMARY KEY,
    user_id        uuid NOT NULL REFERENCES cms.users (id) ON DELETE CASCADE,
    name           text NOT NULL,
    token_hash     bytea NOT NULL UNIQUE,
    -- 表示用の先頭の数文字（秘密の値の一部だが、照合には使えない長さ）
    display_prefix text NOT NULL,
    scopes         text[] NOT NULL CHECK (scopes <@ ARRAY['read', 'write', 'deploy']::text[] AND cardinality(scopes) > 0),
    -- 使えるワークスペース（発行時に選ぶ）
    workspace_ids  uuid[] NOT NULL,
    created_at     timestamptz NOT NULL DEFAULT now(),
    expires_at     timestamptz NOT NULL,
    last_used_at   timestamptz,
    revoked_at     timestamptz
);
CREATE INDEX personal_tokens_user ON cms.personal_tokens (user_id) WHERE revoked_at IS NULL;
