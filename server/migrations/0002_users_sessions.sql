-- 利用者とセッション（共通のスキーマ）。利用者はインスタンス単位で存在する（Q101）。

CREATE TABLE cms.users (
    id                    uuid PRIMARY KEY,
    -- 比較は小文字にそろえて行う
    email                 text NOT NULL UNIQUE CHECK (email = lower(email)),
    -- Argon2id のPHC文字列。応答やログには出さない（INV-AUTH-03）
    password_hash         text,
    is_server_admin       boolean NOT NULL DEFAULT false,
    disabled              boolean NOT NULL DEFAULT false,
    -- ログインの失敗の制限（Q105）: 失敗が続くと、次に試せるまでの待ち時間を段階的に延ばす
    failed_login_count    integer NOT NULL DEFAULT 0,
    login_blocked_until   timestamptz,
    password_changed_at   timestamptz,
    created_at            timestamptz NOT NULL DEFAULT now()
);

-- セッション。セッションIDとCSRFトークンは、DBにはハッシュ（SHA-256）だけを保存する（Q91, Q93）。
CREATE TABLE cms.sessions (
    id                    uuid PRIMARY KEY,
    user_id               uuid NOT NULL REFERENCES cms.users (id) ON DELETE CASCADE,
    token_hash            bytea NOT NULL UNIQUE,
    csrf_hash             bytea NOT NULL,
    created_at            timestamptz NOT NULL DEFAULT now(),
    last_seen_at          timestamptz NOT NULL DEFAULT now(),
    -- 絶対的な期限（ログインからの最大時間）
    expires_at            timestamptz NOT NULL,
    -- 最後に本人確認（ログインか再認証）をした時刻。重い操作の前に確かめる（Q108）
    authenticated_at      timestamptz NOT NULL DEFAULT now(),
    revoked_at            timestamptz
);
CREATE INDEX sessions_user ON cms.sessions (user_id) WHERE revoked_at IS NULL;
