-- 監査ログの外部への書き出し（11-permissions-and-audit.md「外部への書き出し」、Q81、計画の14番）。
-- 記録した時点では送らず、書き出し先ごとに「どこまで送ったか」（記録の時刻とIDの組）を覚えて、順に送る。
-- 7日間続けて送れなければ「停止」にする（停止した書き出し先は、保存期間を過ぎた記録の削除を止めない）。

-- ワークスペースの書き出し先（そのワークスペースの管理者が作る。そのワークスペースの記録だけが出る）
CREATE TABLE cms.audit_sinks (
    id               uuid PRIMARY KEY,
    workspace_id     uuid NOT NULL REFERENCES cms.workspaces (id) ON DELETE CASCADE,
    url              text NOT NULL,
    secret_enc       bytea NOT NULL,
    created_by       uuid REFERENCES cms.users (id) ON DELETE SET NULL,
    created_at       timestamptz NOT NULL DEFAULT now(),
    cursor_at        timestamptz NOT NULL DEFAULT now(),
    cursor_id        uuid NOT NULL DEFAULT '00000000-0000-0000-0000-000000000000',
    failing_since    timestamptz,
    last_error       text,
    last_attempt_at  timestamptz,
    stopped_at       timestamptz
);
CREATE INDEX audit_sinks_workspace ON cms.audit_sinks (workspace_id);

-- インスタンス全体の書き出し先（設定ファイルで決める）の進み具合。sink_key は設定から作る名前（stdout、file:<パス>、webhook:<URL>）
CREATE TABLE cms.audit_export_cursors (
    sink_key         text PRIMARY KEY,
    cursor_at        timestamptz NOT NULL DEFAULT now(),
    cursor_id        uuid NOT NULL DEFAULT '00000000-0000-0000-0000-000000000000',
    failing_since    timestamptz,
    last_error       text,
    last_attempt_at  timestamptz,
    stopped_at       timestamptz
);

-- 記録の順（時刻とID）で読み進めるための索引
CREATE INDEX audit_log_created ON cms.audit_log (created_at, id);
