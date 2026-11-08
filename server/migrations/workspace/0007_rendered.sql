-- 版の本文の変換結果（Markdown と HTML。03-content-and-editing.md「本文」、07-ai-delivery.md）。
-- 公開のときに変換器で作り、版と一緒に書き換えずに残す。本文の抹消では fields と一緒に消す（Q134）。
-- 画像は `yomi-media:<ID>` のまま保存し、配信のときにその時点のメディアのURLに置き換える。
-- HTMLは無害化する前のものを保存し、配信のたびに無害化する（最後の防衛線を出力の直前に置く）。
ALTER TABLE versions
    -- {"<フィールド名>": {"markdown": "...", "html": "..."}}
    ADD COLUMN rendered jsonb,
    -- 変換に使った変換器の版（変わったら作り直す。Q58）
    ADD COLUMN rendered_with text;
