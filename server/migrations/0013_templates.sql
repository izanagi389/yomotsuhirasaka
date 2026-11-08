-- テンプレート（スキーマ）のデプロイと、ワークスペースごとの移行の状態（02-schema-and-delivery-api.md、Q52, Q53, Q130）。

-- デプロイしたテンプレートの版。マニフェストは、検査を通ったものだけを保存する。
-- 書き込みはデプロイの処理（移行・ロールの作成用の接続）だけが行う。普段用のロールは読むだけ。
CREATE TABLE yomi.template_versions (
    template     text NOT NULL,
    version      integer NOT NULL CHECK (version > 0),
    manifest     jsonb NOT NULL,
    deployed_by  uuid REFERENCES yomi.users (id) ON DELETE SET NULL,
    deployed_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (template, version)
);

ALTER TABLE yomi.workspaces
    -- ワークスペースのスキーマの土台のテーブル（記事・版など）の版。サーバーの起動時にそろえる
    ADD COLUMN base_version           integer NOT NULL DEFAULT 0,
    -- 適用済みのテンプレートの版（まだなければ NULL）
    ADD COLUMN schema_version         integer,
    -- none: まだ適用していない、ok: 最新まで適用済み、pending: 移行待ち（Q53）
    ADD COLUMN schema_state           text NOT NULL DEFAULT 'none' CHECK (schema_state IN ('none', 'ok', 'pending')),
    -- 移行待ちのとき、適用しようとした版
    ADD COLUMN schema_target_version  integer,
    -- 移行に失敗した理由（機械的に判別できるコードと、コンテンツタイプ名・フィールド名だけ。記事の内容は書かない）
    ADD COLUMN schema_error           text;
