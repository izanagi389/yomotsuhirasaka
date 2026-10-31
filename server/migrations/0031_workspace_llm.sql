-- ワークスペースの LLM の接続設定の上書き（10-llm-and-budget.md「接続設定の単位」、Q26、M4）。
-- - 上書きを許すかどうかは、サーバー管理者がワークスペースの上限（workspace_limits.llm_override）で決める。既定は許さない。
-- - endpoint は、サーバーの設定の許可の一覧（[[llm.allowed_endpoints]]）の名前。URL は持たない。
-- - api_key は、暗号化の鍵で暗号化した値（AAD はワークスペースIDと用途）。管理APIからは読み出せない（INV-LLM-02）。
ALTER TABLE cms.workspace_limits ADD COLUMN llm_override boolean NOT NULL DEFAULT false;

CREATE TABLE cms.workspace_llm (
    workspace_id        uuid PRIMARY KEY REFERENCES cms.workspaces (id) ON DELETE CASCADE,
    endpoint            text NOT NULL,
    api_key             bytea NOT NULL,
    api_key_updated_at  timestamptz NOT NULL DEFAULT now(),
    chat_model          text,
    chat_input_price    double precision CHECK (chat_input_price >= 0),
    chat_output_price   double precision CHECK (chat_output_price >= 0),
    updated_by          uuid REFERENCES cms.users (id) ON DELETE SET NULL,
    updated_at          timestamptz NOT NULL DEFAULT now()
);
