-- 重い権限「テンプレートの切り替え」（02-schema-and-delivery-api.md「テンプレートの切り替え」、2026-11-11）。
-- どのロールにも既定では付けず、付与できるのはサーバー管理者だけ（サーバーのコードで確かめる）。
ALTER TABLE yomi.memberships DROP CONSTRAINT memberships_grants_check;
ALTER TABLE yomi.memberships ADD CONSTRAINT memberships_grants_check
    CHECK (grants <@ ARRAY['viewMembers', 'viewQuestionLog', 'manageLlm', 'policySwitches', 'corrections', 'takeOverEditing', 'switchTemplate']::text[]);
