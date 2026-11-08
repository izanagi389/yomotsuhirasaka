-- 利用者の表示名（レビューする人の候補や、レビューの履歴の「誰が」に使う。11-permissions-and-audit.md、計画の8番）。
-- 本人だけが決める。未設定なら、表示する側がメールアドレスで代える。
-- display_name_suggestion は、SSOのIdPが返した名前（入力欄の初期値。本人が保存するまで表示名にはしない）。本人にだけ返す。
ALTER TABLE yomi.users
    ADD COLUMN display_name text CHECK (display_name IS NULL OR char_length(display_name) BETWEEN 1 AND 50),
    ADD COLUMN display_name_suggestion text
        CHECK (display_name_suggestion IS NULL OR char_length(display_name_suggestion) BETWEEN 1 AND 50);
