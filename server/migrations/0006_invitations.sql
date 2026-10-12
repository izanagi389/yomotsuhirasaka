-- 招待（Q104, Q121, Q193）と、所属者の一覧の閲覧の権限（Q190）。

-- 招待。相手が既存の利用者でも、本人が承諾するまで所属させない（INV-TEN-05）。
-- トークンはランダムで一度だけ使え、短命。DBにはハッシュだけを保存する。
CREATE TABLE cms.invitations (
    id            uuid PRIMARY KEY,
    workspace_id  uuid NOT NULL REFERENCES cms.workspaces (id) ON DELETE CASCADE,
    email         text NOT NULL CHECK (email = lower(email)),
    role          text NOT NULL CHECK (role IN ('admin', 'editor', 'author', 'viewer')),
    token_hash    bytea NOT NULL UNIQUE,
    invited_by    uuid REFERENCES cms.users (id) ON DELETE SET NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    expires_at    timestamptz NOT NULL,
    accepted_at   timestamptz,
    revoked_at    timestamptz
);
CREATE INDEX invitations_pending ON cms.invitations (workspace_id)
    WHERE accepted_at IS NULL AND revoked_at IS NULL;

-- 管理者が個別に与える、所属者の一覧の閲覧の権限（管理者は常に見られる）。
ALTER TABLE cms.memberships
    ADD COLUMN can_view_members boolean NOT NULL DEFAULT false;
