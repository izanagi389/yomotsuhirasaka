-- パスキー（多要素認証の方法の1つ。13-security.md「ログイン」）。

-- 登録したパスキー。公開鍵と署名の回数などを webauthn-rs の形（JSON）で保存する。秘密の値は含まない。
-- 同じ資格情報のIDを、別の利用者に登録させない（UNIQUE）。
CREATE TABLE yomi.passkeys (
    id             uuid PRIMARY KEY,
    user_id        uuid NOT NULL REFERENCES yomi.users (id) ON DELETE CASCADE,
    name           text NOT NULL,
    credential_id  bytea NOT NULL UNIQUE,
    passkey        jsonb NOT NULL,
    created_at     timestamptz NOT NULL DEFAULT now(),
    last_used_at   timestamptz
);
CREATE INDEX passkeys_user ON yomi.passkeys (user_id);

-- 登録・ログイン・再認証の途中の状態（チャレンジ）。サーバー側にだけ置き、一度だけ使う。
-- セッション（多要素認証の途中のものを含む）ごとに、用途ごとに1つ。
CREATE TABLE yomi.webauthn_challenges (
    session_id   uuid NOT NULL REFERENCES yomi.sessions (id) ON DELETE CASCADE,
    purpose      text NOT NULL CHECK (purpose IN ('register', 'login', 'reauth')),
    user_id      uuid NOT NULL REFERENCES yomi.users (id) ON DELETE CASCADE,
    state        jsonb NOT NULL,
    -- 登録のときに付ける名前
    name         text,
    expires_at   timestamptz NOT NULL,
    PRIMARY KEY (session_id, purpose)
);
CREATE INDEX webauthn_challenges_expires ON yomi.webauthn_challenges (expires_at);
