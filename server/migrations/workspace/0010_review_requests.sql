-- レビュー依頼（03-content-and-editing.md「レビュー（Q77）」、計画の8番）。
-- 1段階だけ: 依頼 → 差し戻し（コメント必須）・取り下げ・公開のどれかで閉じる。承認という段階は作らない。
-- 開いている依頼は記事ごとに1つ。依頼した時点の下書きのリビジョンを残し、その後の編集は「依頼後に変更あり」として示す。
-- 利用者のIDは共通のスキーマ（yomi.users）のもので、ここでは参照の制約を付けない（他の表と同じ）。
CREATE TABLE review_requests (
    id                  uuid PRIMARY KEY,
    entry_id            uuid NOT NULL REFERENCES entries (id),
    requested_by        uuid NOT NULL,
    requested_revision  bigint NOT NULL,
    -- 見てほしい人（任意。目印だけで、他の公開できる人も差し戻し・公開できる）
    reviewer            uuid,
    state               text NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'rejected', 'withdrawn', 'published')),
    -- 差し戻しのコメント（差し戻しのときだけ。監査ログには書かない）
    comment             text CHECK (comment IS NULL OR char_length(comment) BETWEEN 1 AND 2000),
    created_at          timestamptz NOT NULL DEFAULT now(),
    closed_at           timestamptz,
    closed_by           uuid,
    CHECK ((state = 'rejected') = (comment IS NOT NULL)),
    CHECK ((state = 'open') = (closed_at IS NULL))
);
CREATE UNIQUE INDEX review_requests_open ON review_requests (entry_id) WHERE state = 'open';
CREATE INDEX review_requests_entry ON review_requests (entry_id, created_at);
