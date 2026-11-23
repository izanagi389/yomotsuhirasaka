-- テンプレートの版をやめる（02-schema-and-delivery-api.md「移行」、2026-11-23 の持ち主の判断）。
-- サーバーはテンプレートごとに最新のマニフェストを1つだけ持ち、デプロイはテンプレートを使うすべてのワークスペースに、
-- 全部適用するか全部やめるかにする。ワークスペースは「まだ適用していない（none）」か「適用済み（ok）」だけを持つ。

-- 移行待ちのワークスペース（または最新の版に追いついていないワークスペース）があれば、ここで止める。
-- 最新のマニフェストだけを残すと、そのワークスペースの今のスキーマがわからなくなるため。古いサーバーで解消してから上げる。
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM yomi.workspaces w
        WHERE w.schema_state = 'pending'
           OR (w.schema_version IS NOT NULL AND w.schema_version <> (
                SELECT max(v.version) FROM yomi.template_versions v WHERE v.template = w.template))
    ) THEN
        RAISE EXCEPTION 'some workspaces are waiting for a schema migration; resolve them with the previous server version (fix the registered queries or the entries, then deploy again) before upgrading';
    END IF;
END $$;

-- テンプレートごとの最新のマニフェスト。書き込みはデプロイの処理（移行・ロールの作成用の接続）だけが行う。
CREATE TABLE yomi.templates (
    name         text PRIMARY KEY,
    manifest     jsonb NOT NULL,
    deployed_by  uuid REFERENCES yomi.users (id) ON DELETE SET NULL,
    -- 最後に内容を変えてデプロイした日時（利用者に見せる。配信のスキーマのキャッシュのキーにも使う）
    deployed_at  timestamptz NOT NULL DEFAULT now()
);

INSERT INTO yomi.templates (name, manifest, deployed_by, deployed_at)
SELECT DISTINCT ON (template) template, manifest, deployed_by, deployed_at
FROM yomi.template_versions
ORDER BY template, version DESC;

DROP TABLE yomi.template_versions;

ALTER TABLE yomi.workspaces
    DROP COLUMN schema_version,
    DROP COLUMN schema_target_version,
    DROP COLUMN schema_error;
UPDATE yomi.workspaces SET schema_state = 'none' WHERE schema_state NOT IN ('none', 'ok');
ALTER TABLE yomi.workspaces DROP CONSTRAINT workspaces_schema_state_check;
ALTER TABLE yomi.workspaces ADD CONSTRAINT workspaces_schema_state_check CHECK (schema_state IN ('none', 'ok'));
