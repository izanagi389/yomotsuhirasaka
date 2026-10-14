-- 外部のIdP（OIDC/SSO）のアカウントと、CMSの利用者のひも付け（13-security.md「OIDC/SSO」、Q103, Q126）。
-- 発行者（iss）と利用者の識別子（sub）の組で1人を指す。メールアドレスでは結び付けない。
-- ひも付けを作る処理（OIDCのログインとひも付け）はM1の最後に入れる。それまで、この表は空のまま。
CREATE TABLE cms.external_identities (
    id             uuid PRIMARY KEY,
    user_id        uuid NOT NULL REFERENCES cms.users (id) ON DELETE CASCADE,
    issuer         text NOT NULL,
    subject        text NOT NULL,
    linked_at      timestamptz NOT NULL DEFAULT now(),
    last_login_at  timestamptz,
    UNIQUE (issuer, subject)
);
CREATE INDEX external_identities_user ON cms.external_identities (user_id);
