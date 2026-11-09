-- 本人確認の2段目（多要素認証のコードと再認証）の失敗の記録。パスワードの失敗とは別に数える。
-- パスワードの成功で2段目の失敗が消えないようにするため。また、パスワードを知らない人が再認証を止められないようにするため。
ALTER TABLE yomi.users
    ADD COLUMN second_factor_failed_count  integer NOT NULL DEFAULT 0,
    ADD COLUMN second_factor_blocked_until timestamptz;
