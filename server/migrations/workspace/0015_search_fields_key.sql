-- チャンクを作ったときの「検索の対象のフィールド」の決め方の要約（08-search-and-related.md、INV-SRC-03）。
-- 検索のたびに今のスキーマの決め方と照合し、合わないチャンク（テンプレートで対象を外した直後など）は使わない。
-- 既にある行は空にしておく（作り直しのジョブが、今の決め方で書き直すまで使わない）。
ALTER TABLE search_chunks ADD COLUMN fields_key text NOT NULL DEFAULT '';
