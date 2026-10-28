-- 関連記事（08-search-and-related.md「関連記事」、Q60〜Q62、M3）。
-- - search_entry_vectors: 記事全体のベクトル（公開中の版のチャンクの埋め込みの平均を、長さ1にそろえたもの）。モデルごと。
-- - related_entries: 記事ごとの関連記事の候補（似ている順。補正の後の点数と一緒に）。公開のときと毎日の一括計算のときに作る。
--   返すときに、公開状態・ロケール・除外を毎回照合する（ここにあっても、見せてよいとは限らない。原則3、INV-REL-01）。
-- - related_settings: 編集者が決める固定（順番付き）と除外。どちらも翻訳グループを指し、返すときに同じロケールの記事へ解決する。
CREATE TABLE search_entry_vectors (
    entry_id    uuid NOT NULL,
    version_id  uuid NOT NULL,
    model       text NOT NULL,
    dimensions  integer NOT NULL,
    vector      real[] NOT NULL,
    updated_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (entry_id, model, dimensions)
);

CREATE TABLE related_entries (
    entry_id     uuid PRIMARY KEY,
    candidates   uuid[] NOT NULL,
    scores       real[] NOT NULL,
    computed_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE related_settings (
    entry_id    uuid PRIMARY KEY,
    pinned      uuid[] NOT NULL DEFAULT '{}',
    excluded    uuid[] NOT NULL DEFAULT '{}',
    updated_by  uuid,
    updated_at  timestamptz NOT NULL DEFAULT now()
);
