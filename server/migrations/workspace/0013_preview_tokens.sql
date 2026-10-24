-- プレビュー用トークン（03-content-and-editing.md「プレビュー（Q72）」、計画の13番、INV-PRV-01, 02）。
-- 1つの記事の下書きだけを読める。値は発行のときに一度だけ渡し、ここにはハッシュ（SHA-256）だけを残す。
-- 有効期間（最長10分）を過ぎたもの・失効させたものは使えない。期限切れの行は、発行のときに古いものから消す。
CREATE TABLE preview_tokens (
    id          uuid PRIMARY KEY,
    entry_id    uuid NOT NULL REFERENCES entries (id),
    token_hash  bytea NOT NULL UNIQUE,
    created_by  uuid NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    expires_at  timestamptz NOT NULL,
    revoked_at  timestamptz
);
CREATE INDEX preview_tokens_entry ON preview_tokens (entry_id);
