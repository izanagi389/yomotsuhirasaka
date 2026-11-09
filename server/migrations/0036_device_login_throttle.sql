-- 知っている端末のログインの失敗を、アカウントとは別に数える（13-security.md「ログインの失敗の制限」、第三者の確認の R9）。
-- パスワードを知らない人が、失敗を送り続けて本人を締め出せないようにする。数え方はアカウントのものと同じ。
ALTER TABLE yomi.known_devices
    ADD COLUMN failed_login_count  integer NOT NULL DEFAULT 0,
    ADD COLUMN login_blocked_until timestamptz;
