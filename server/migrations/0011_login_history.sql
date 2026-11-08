-- 新しい端末の判定と、ログインの履歴（13-security.md「本人への通知」、Q171, Q175, Q176, Q181）。

-- ログインに成功した端末。端末のCookieの値（ランダムなID）は、DBにはハッシュだけを保存する。
-- 新しい端末の通知だけに使い、多要素認証の省略などには使わない（Q181）。
CREATE TABLE yomi.known_devices (
    user_id       uuid NOT NULL REFERENCES yomi.users (id) ON DELETE CASCADE,
    device_hash   bytea NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    last_seen_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, device_hash)
);
CREATE INDEX known_devices_last_seen ON yomi.known_devices (last_seen_at);

-- ログインの履歴。保存期間（既定90日）を過ぎたものは定期的に削除する（INV-AUD-07）。
CREATE TABLE yomi.login_history (
    id            uuid PRIMARY KEY,
    user_id       uuid NOT NULL REFERENCES yomi.users (id) ON DELETE CASCADE,
    created_at    timestamptz NOT NULL DEFAULT now(),
    ip            inet NOT NULL,
    -- ブラウザの種類（User-Agent をそのまま保存せず、決まった名前の1つにまとめる）
    browser       text NOT NULL,
    -- 国（GeoIPのデータがある場合だけ。今は常に NULL。Q175）
    country       text,
    new_device    boolean NOT NULL
);
CREATE INDEX login_history_user ON yomi.login_history (user_id, created_at DESC);
CREATE INDEX login_history_created ON yomi.login_history (created_at);
