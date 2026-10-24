-- 予約公開（03-content-and-editing.md「予約公開（Q78）」、Q133、計画の9番）。
-- 予約のときに公開と同じ検査と計算を済ませ、結果（予約版）を prepared に保存する。予約の時刻に、それを版にして公開する。
-- 開いている予約（pending）は記事ごとに1つ。公開できなかったときは last_error と next_attempt_at を残して、やり直す。
-- 閉じたら prepared を消す（予約版の中身を残し続けない）。
CREATE TABLE schedules (
    id               uuid PRIMARY KEY,
    entry_id         uuid NOT NULL REFERENCES entries (id),
    publish_at       timestamptz NOT NULL,
    -- 予約版のもとにした下書きのリビジョン（その後の編集との食い違いを示す）
    draft_revision   bigint NOT NULL,
    -- 予約のときのテンプレートの版（版に記録する）
    schema_version   integer NOT NULL,
    prepared         jsonb,
    scheduled_by     uuid NOT NULL,
    created_at       timestamptz NOT NULL DEFAULT now(),
    state            text NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'published', 'cancelled', 'failed')),
    -- 取り消しの理由（cancelled のとき）
    reason           text CHECK (reason IS NULL OR reason IN ('cancelled', 'replaced', 'corrected', 'published', 'deleted')),
    last_error       text,
    attempts         integer NOT NULL DEFAULT 0,
    next_attempt_at  timestamptz,
    closed_at        timestamptz,
    closed_by        uuid,
    CHECK ((state = 'pending') = (closed_at IS NULL)),
    CHECK ((state = 'pending') = (prepared IS NOT NULL)),
    CHECK ((state = 'cancelled') = (reason IS NOT NULL))
);
CREATE UNIQUE INDEX schedules_open ON schedules (entry_id) WHERE state = 'pending';
CREATE INDEX schedules_entry ON schedules (entry_id, created_at);

-- レビュー依頼は、予約した時点で「予約済み」として閉じる
ALTER TABLE review_requests DROP CONSTRAINT review_requests_state_check;
ALTER TABLE review_requests ADD CONSTRAINT review_requests_state_check
    CHECK (state IN ('open', 'rejected', 'withdrawn', 'published', 'scheduled'));
