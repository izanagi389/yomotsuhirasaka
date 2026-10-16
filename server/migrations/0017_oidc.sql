-- OIDC/SSOでのログイン（13-security.md「OIDC/SSO」、Q103, Q126, Q131）。

-- ログインの途中の状態（IdPへ送ってから戻るまで。10分で失効）。
-- state と nonce と PKCE の検証用の値をサーバーに持ち、ブラウザには短命のCookie（SameSite=Lax）の値だけを渡す。
-- 戻ってきたとき、Cookie の値と state の両方が一致した行だけを使い、使ったら消す。
CREATE TABLE cms.oidc_logins (
    id              uuid PRIMARY KEY,
    -- 短命のCookieの値のハッシュ（state を、始めたブラウザに結び付ける）
    browser_hash    bytea NOT NULL,
    state_hash      bytea NOT NULL UNIQUE,
    nonce           text NOT NULL,
    -- PKCE の検証用の値（暗号化して保存）
    pkce_verifier   bytea NOT NULL,
    -- login: ログイン、invite: 招待の承諾、link: ログイン中の本人のひも付け、reauth: 再認証
    purpose         text NOT NULL CHECK (purpose IN ('login', 'invite', 'link', 'reauth')),
    -- link・reauth の本人と、そのセッション
    user_id         uuid REFERENCES cms.users (id) ON DELETE CASCADE,
    session_id      uuid REFERENCES cms.sessions (id) ON DELETE CASCADE,
    -- invite の招待
    invitation_id   uuid REFERENCES cms.invitations (id) ON DELETE CASCADE,
    created_at      timestamptz NOT NULL DEFAULT now(),
    expires_at      timestamptz NOT NULL
);
CREATE INDEX oidc_logins_expires ON cms.oidc_logins (expires_at);

-- セッションの認証の方法と、最後の再認証の方法。
-- SSOでログインした管理者は、CMS側のパスキーでの再認証だけを有効とする（Q131）。
-- SSOのログインはIdPからのリダイレクトで終わるので、CSRFトークンは、戻った画面が一度だけ受け取る（csrf_handoff）。
ALTER TABLE cms.sessions
    ADD COLUMN auth_method text NOT NULL DEFAULT 'password' CHECK (auth_method IN ('password', 'oidc')),
    ADD COLUMN reauth_method text CHECK (reauth_method IN ('password', 'totp', 'recovery', 'passkey', 'oidc')),
    ADD COLUMN csrf_handoff boolean NOT NULL DEFAULT false;

-- 1人の利用者は、同じ発行者のアカウントを1つだけひも付ける
ALTER TABLE cms.external_identities ADD CONSTRAINT external_identities_one_per_issuer UNIQUE (user_id, issuer);
