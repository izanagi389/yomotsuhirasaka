-- Webhook（01-architecture.md「Webhook」、13-security.md「外部への送信」「Webhookの署名」）。ワークスペースごと。
-- 共通のスキーマに置き、ワークスペースのロールからは触れない（配信APIのキーと同じ）。
--
-- secret_enc: 署名用の秘密。暗号化の鍵で暗号化して保存する（13-security.md「鍵」）。発行時にだけ利用者に渡す。
-- previous_secret_enc: 秘密を入れ替えたときの古い秘密。previous_secret_until まで、新旧の両方で署名する。
CREATE TABLE cms.webhooks (
    id                     uuid PRIMARY KEY,
    workspace_id           uuid NOT NULL REFERENCES cms.workspaces (id) ON DELETE CASCADE,
    url                    text NOT NULL,
    secret_enc             bytea NOT NULL,
    previous_secret_enc    bytea,
    previous_secret_until  timestamptz,
    created_by             uuid REFERENCES cms.users (id) ON DELETE SET NULL,
    created_at             timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX webhooks_workspace ON cms.webhooks (workspace_id);

-- 送信の記録。相手の応答は状態コードだけを残し、本文やヘッダーは残さない（Q112、INV-WH-04）。
-- Webhookの本文も残さない（記事の本文は含まないが、送った内容はジョブの暗号化された中身にだけ置く）。
CREATE TABLE cms.webhook_deliveries (
    id               uuid PRIMARY KEY,
    webhook_id       uuid NOT NULL REFERENCES cms.webhooks (id) ON DELETE CASCADE,
    workspace_id     uuid NOT NULL REFERENCES cms.workspaces (id) ON DELETE CASCADE,
    event_id         uuid NOT NULL,
    event_type       text NOT NULL,
    entry_id         uuid,
    state            text NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'delivered', 'failed')),
    attempts         integer NOT NULL DEFAULT 0,
    -- 最後の試行で相手が返した状態コード（届かなければ NULL）
    last_status      integer,
    -- 最後の試行の失敗の種類（例: internal_destination, timeout, http_status）
    last_error       text,
    created_at       timestamptz NOT NULL DEFAULT now(),
    last_attempt_at  timestamptz
);
CREATE INDEX webhook_deliveries_webhook ON cms.webhook_deliveries (webhook_id, created_at DESC);
