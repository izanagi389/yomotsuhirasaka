-- 公開・非公開・削除と、後から取り返せない記録（03-content-and-editing.md, 05-urls-and-redirects.md, 06-corrections-and-freshness.md）。

-- 記事の公開URL（最後に公開したときに計算した値。非公開・削除の後も残し、次に公開したときの変更の記録に使う。Q39）
ALTER TABLE entries ADD COLUMN url text;
-- 公開中の記事どうしで、公開URLは重複できない
CREATE UNIQUE INDEX entries_published_url ON entries (url) WHERE published_version_id IS NOT NULL;

-- 版のメタデータ（本文を抹消しても残す。Q134）
-- url: この版を公開したときのURL
-- anchors: この版の本文にあったアンカーID（削除済みアンカーを後から求められるように。Q47）
-- affected_anchors: 「訂正として公開」したとき、前の版から変わった・消えた・増えたセクションのアンカーID（Q42）
-- affects_lead: 「訂正として公開」したとき、最初の見出しより前の部分が変わったか
ALTER TABLE versions
    ADD COLUMN url text,
    ADD COLUMN anchors text[] NOT NULL DEFAULT '{}',
    ADD COLUMN affected_anchors text[],
    ADD COLUMN affects_lead boolean;

-- 公開URLの変更履歴（Q40）。追記だけで、書き換えない。
CREATE TABLE url_changes (
    id          uuid PRIMARY KEY,
    entry_id    uuid NOT NULL REFERENCES entries (id),
    from_url    text NOT NULL,
    to_url      text NOT NULL,
    -- 変更を起こした公開の版
    version_id  uuid REFERENCES versions (id),
    changed_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX url_changes_entry ON url_changes (entry_id, changed_at);
CREATE INDEX url_changes_from ON url_changes (from_url);
