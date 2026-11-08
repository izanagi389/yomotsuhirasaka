-- プレビューを許すサイトのオリジン（03-content-and-editing.md「プレビュー（Q72）」、13-security.md「ライブプレビュー」、計画の13番）。
-- 記事の画面の CSP の frame-src と、ライブプレビューのメッセージの送り元の照合（INV-PRV-03）に使う。
-- 値は正規化したオリジン（`https://example.com`、既定のポートは書かない）。http は localhost だけ。最大10件。
ALTER TABLE yomi.workspace_settings ADD COLUMN preview_origins text[] NOT NULL DEFAULT '{}';
