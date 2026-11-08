-- パスワードと多要素認証のリセットのトークン（Q104, Q117）。
-- ランダムで一度だけ使え、短命。DBにはハッシュだけを保存する。
-- サーバー管理者のリセットはネットワーク越しには行わない（INV-AUTH-10。サーバー上のCLIで行う）。
CREATE TABLE yomi.reset_tokens (
    id          uuid PRIMARY KEY,
    user_id     uuid NOT NULL REFERENCES yomi.users (id) ON DELETE CASCADE,
    kind        text NOT NULL CHECK (kind IN ('password', 'mfa')),
    token_hash  bytea NOT NULL UNIQUE,
    -- 発行した管理者（本人の要求なら NULL）
    issued_by   uuid REFERENCES yomi.users (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    expires_at  timestamptz NOT NULL,
    used_at     timestamptz,
    revoked_at  timestamptz
);
CREATE INDEX reset_tokens_user ON yomi.reset_tokens (user_id, kind) WHERE used_at IS NULL AND revoked_at IS NULL;
