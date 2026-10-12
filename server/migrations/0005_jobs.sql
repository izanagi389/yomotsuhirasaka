-- ジョブのキュー（01-architecture.md: PostgreSQLに置き、SKIP LOCKED で取り出す）。
-- 外部との通信（メールなど）は、同期の処理ではなくジョブで、リトライ付きで行う（設計の原則5）。
--
-- payload_enc は暗号化の鍵で暗号化した中身（宛先、本文、リンクのトークンなど）。
-- 終わったジョブは消し、送れなかったジョブは中身を消して記録だけを残す。

CREATE TABLE cms.jobs (
    id            uuid PRIMARY KEY,
    -- インスタンス全体のジョブは NULL
    workspace_id  uuid REFERENCES cms.workspaces (id) ON DELETE CASCADE,
    kind          text NOT NULL,
    payload_enc   bytea,
    run_at        timestamptz NOT NULL DEFAULT now(),
    attempts      integer NOT NULL DEFAULT 0,
    max_attempts  integer NOT NULL,
    locked_until  timestamptz,
    -- 失敗の種類だけ（相手の応答の本文や宛先は書かない）
    last_error    text,
    failed_at     timestamptz,
    created_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX jobs_ready ON cms.jobs (run_at) WHERE failed_at IS NULL;
