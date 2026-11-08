-- 配信APIのキーのスコープに `answer`（回答API）を足す（read と分ける。お金がかかるため）。
ALTER TABLE yomi.api_keys DROP CONSTRAINT api_keys_scopes_check;
ALTER TABLE yomi.api_keys ADD CONSTRAINT api_keys_scopes_check
    CHECK (scopes <@ ARRAY['read', 'register', 'answer']::text[] AND cardinality(scopes) > 0);
