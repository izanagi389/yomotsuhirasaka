-- 多要素認証（TOTPと復旧コード）とログインの途中の状態（Q90, Q107, Q108）。

-- TOTPの秘密は、暗号化の鍵（keys.rs）で暗号化して保存する（13-security.md「鍵」）。
-- totp_pending_secret_enc は設定の途中（コードの確認の前）の秘密。確認できたら totp_secret_enc に移す。
ALTER TABLE cms.users
    ADD COLUMN totp_secret_enc          bytea,
    ADD COLUMN totp_pending_secret_enc  bytea,
    ADD COLUMN totp_enabled_at          timestamptz,
    -- 最後に受け付けたコードの時間の区切り（同じコードを二度受け付けないため。RFC 6238 5.2）
    ADD COLUMN totp_last_step           bigint,
    ADD CONSTRAINT users_totp_consistent CHECK ((totp_secret_enc IS NULL) = (totp_enabled_at IS NULL));

-- 復旧コード。一度だけ使える。DBにはハッシュだけを保存する。
CREATE TABLE cms.recovery_codes (
    id          uuid PRIMARY KEY,
    user_id     uuid NOT NULL REFERENCES cms.users (id) ON DELETE CASCADE,
    code_hash   bytea NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    used_at     timestamptz,
    UNIQUE (user_id, code_hash)
);

-- パスワードは通ったが、多要素認証のコードをまだ受け取っていないセッション。
-- このセッションでは、多要素認証のコードを送ること以外は何もできない。
ALTER TABLE cms.sessions
    ADD COLUMN mfa_pending boolean NOT NULL DEFAULT false;
