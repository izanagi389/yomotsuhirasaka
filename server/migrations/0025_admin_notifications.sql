-- 管理者への通知（13-security.md「管理者への通知」、Q174、計画の15番）。
-- 通知は監査ログの記録に印（notify）を付けたもの。監査ログの書き出し（インスタンス全体・ワークスペース）と、
-- 管理画面の「やること」（ワークスペース）・サーバー管理者の画面（インスタンス）で知らせる。
-- details は急増の通知（alert.*。インスタンス単位）だけが使う: 件数・期間と、件数の多いIPアドレス・利用者・メールアドレス。
ALTER TABLE yomi.audit_log ADD COLUMN notify boolean NOT NULL DEFAULT false;
ALTER TABLE yomi.audit_log ADD COLUMN details jsonb;
CREATE INDEX audit_log_notify ON yomi.audit_log (workspace_id, created_at) WHERE notify;

-- 急増を数えるための件数（1分ごと。サーバーのプロセスが10秒ごとにまとめて書く）。1日で消す。
CREATE TABLE yomi.alert_events (
    kind       text NOT NULL,
    bucket_at  timestamptz NOT NULL,
    ip         text,
    user_id    uuid,
    count      bigint NOT NULL
);
CREATE UNIQUE INDEX alert_events_key ON yomi.alert_events
    (kind, bucket_at, coalesce(ip, ''), coalesce(user_id, '00000000-0000-0000-0000-000000000000'));

-- 通知を「確認した」記録（1人ずつ）。
CREATE TABLE yomi.notification_reads (
    user_id   uuid NOT NULL REFERENCES yomi.users (id) ON DELETE CASCADE,
    audit_id  uuid NOT NULL,
    read_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, audit_id)
);
