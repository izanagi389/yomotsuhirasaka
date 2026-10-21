-- URL生成関数の依存（05-urls-and-redirects.md「計算と保存」、Q39）。
-- 記事のURLを計算したとき、URL生成関数がホスト関数で読もうとした記事のIDを記録する（見つからなかったものも含む）。
-- 依存先の記事が公開・非公開・削除されたり、URLが変わったりしたら、依存する記事のURLを再計算する。
-- 依存先は他の記事の削除後も残しうる（再計算のときに読み直すため）ので、外部キーは張らない。
CREATE TABLE url_dependencies (
    entry_id    uuid NOT NULL REFERENCES entries (id),
    depends_on  uuid NOT NULL,
    PRIMARY KEY (entry_id, depends_on)
);
CREATE INDEX url_dependencies_depends_on ON url_dependencies (depends_on);
