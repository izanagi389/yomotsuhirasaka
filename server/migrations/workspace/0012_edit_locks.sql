-- 編集のロック（ソフトロック。03-content-and-editing.md「同時編集（Q73）」、計画の10番）。
-- ロケールごとの記事に1つ。記事の画面（タブ）ごとのID（lock_id）で持ち、生存の知らせで期限を延ばす。期限が過ぎたら無いものとして扱う。
-- 下書きの保存は、ほかの画面の有効なロックがあれば断る（INV-LOCK-01）。保存ではロックを取らない。
CREATE TABLE edit_locks (
    entry_id     uuid PRIMARY KEY REFERENCES entries (id),
    lock_id      uuid NOT NULL UNIQUE,
    user_id      uuid NOT NULL,
    session_id   uuid NOT NULL,
    acquired_at  timestamptz NOT NULL DEFAULT now(),
    expires_at   timestamptz NOT NULL
);
