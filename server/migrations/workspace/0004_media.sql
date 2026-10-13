-- メディア（13-security.md「メディア」、Q96）。
-- ファイルはローカルのファイルシステムに、ランダムな名前で置く。利用者が指定したファイル名は保存しない。
CREATE TABLE media (
    id            uuid PRIMARY KEY,
    -- 保存したファイルの名前（ランダムな32桁の16進数と拡張子）。配信のURLにも使う
    file          text NOT NULL UNIQUE CHECK (file ~ '^[0-9a-f]{32}\.(jpg|png|gif|webp)$'),
    content_type  text NOT NULL,
    -- 保存したファイル（エンコードし直した後）の大きさ
    bytes         bigint NOT NULL CHECK (bytes >= 0),
    width         integer NOT NULL,
    height        integer NOT NULL,
    created_by    uuid,
    created_at    timestamptz NOT NULL DEFAULT now(),
    -- 削除したら日時を入れ、ファイルを消す
    deleted_at    timestamptz
);
CREATE INDEX media_live ON media (created_at) WHERE deleted_at IS NULL;
