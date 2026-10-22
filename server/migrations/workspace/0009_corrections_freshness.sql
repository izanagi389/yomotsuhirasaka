-- 訂正の説明文と、鮮度（最終確認日と確認期限）（06-corrections-and-freshness.md、Q42〜Q48, Q63）。

-- 訂正の説明文の定型文（ワークスペースごと、ロケールごと。管理者が編集する）。
-- kind = 'choice': 訂正として公開するときに選べる文。kind = 'fallback': 説明を書いていない訂正に、訪問者向けに出す文
CREATE TABLE correction_templates (
    id        uuid PRIMARY KEY,
    locale    text NOT NULL,
    kind      text NOT NULL CHECK (kind IN ('choice', 'fallback')),
    text      text NOT NULL,
    position  integer NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX correction_templates_fallback ON correction_templates (locale) WHERE kind = 'fallback';
-- 初期値（日本語・英語）。管理者が書き換えられる
INSERT INTO correction_templates (id, locale, kind, text, position) VALUES
    (gen_random_uuid(), 'ja', 'fallback', '訂正されました。', 0),
    (gen_random_uuid(), 'ja', 'choice', '事実の誤りを訂正しました。', 1),
    (gen_random_uuid(), 'ja', 'choice', '数値の誤りを訂正しました。', 2),
    (gen_random_uuid(), 'ja', 'choice', 'リンク先の誤りを訂正しました。', 3),
    (gen_random_uuid(), 'ja', 'choice', '誤解を招く表現を改めました。', 4),
    (gen_random_uuid(), 'en', 'fallback', 'This article was corrected.', 0),
    (gen_random_uuid(), 'en', 'choice', 'We corrected a factual error.', 1),
    (gen_random_uuid(), 'en', 'choice', 'We corrected an incorrect figure.', 2),
    (gen_random_uuid(), 'en', 'choice', 'We corrected an incorrect link.', 3),
    (gen_random_uuid(), 'en', 'choice', 'We revised misleading wording.', 4);

-- 訂正の説明（「訂正として公開」した版ごと）。定型文はその時点の文を写して保存する（定型文を直しても変わらない）
CREATE TABLE correction_notes (
    version_id  uuid PRIMARY KEY REFERENCES versions (id),
    text        text NOT NULL
);
-- 説明の編集の履歴（管理画面の中だけで使う。配信APIには出さない。INV-COR-04）
CREATE TABLE correction_note_history (
    id          uuid PRIMARY KEY,
    version_id  uuid NOT NULL REFERENCES versions (id),
    text        text NOT NULL,
    edited_by   uuid,
    edited_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX correction_note_history_version ON correction_note_history (version_id, edited_at);

-- 鮮度。最終確認日は「確認済み」の操作でだけ進める（NULL なら最初に公開した日）。
-- 確認期限の日数は、記事ごとに上書きできる（NULL ならスキーマの既定、review_disabled なら期限なし）
ALTER TABLE entries
    ADD COLUMN last_verified_at timestamptz,
    ADD COLUMN review_interval_days integer CHECK (review_interval_days IS NULL OR review_interval_days > 0),
    ADD COLUMN review_disabled boolean NOT NULL DEFAULT false;
-- 「確認済み」の記録（確認者とメモは管理画面の中だけで使う。配信APIには最終確認日だけを出す。INV-FRS-01）
CREATE TABLE verifications (
    id           uuid PRIMARY KEY,
    entry_id     uuid NOT NULL REFERENCES entries (id),
    verified_by  uuid,
    memo         text NOT NULL DEFAULT '',
    verified_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX verifications_entry ON verifications (entry_id, verified_at);
