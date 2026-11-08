-- 個人に追加で付与する権限（11-permissions-and-audit.md「ロールと権限」、Q76, Q190）。
-- ロールの既定に加えて、管理者が所属者ごとに付与する。M1の「所属者の一覧の閲覧」（can_view_members）もここに移す。
-- 名前はサーバーのコード（access.rs の Permission）と同じ。知らない名前は入れられない。
ALTER TABLE yomi.memberships
    ADD COLUMN grants text[] NOT NULL DEFAULT '{}'
        CHECK (grants <@ ARRAY['viewMembers', 'viewQuestionLog', 'manageLlm', 'policySwitches', 'corrections', 'takeOverEditing']::text[]);
UPDATE yomi.memberships SET grants = ARRAY['viewMembers'] WHERE can_view_members;
ALTER TABLE yomi.memberships DROP COLUMN can_view_members;
