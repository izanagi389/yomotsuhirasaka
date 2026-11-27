-- 配信APIの一覧の絞り込みと並べ替えに使う、公開した版の値の写し（02-schema-and-delivery-api.md「配信API」）。
-- 絞り込みのたびに本文を含む fields 全体を読むと、記事が多いときに遅い（10万件で数秒）。そこで、短い値だけを
-- 小さな列に写す。生成列なので、公開・本文の抹消（fields を NULL にする）・移行のどれでも、DBが fields から作り直す。
-- 情報源は fields のまま（原則2）。この列は絞り込みと並べ替えにだけ使い、配信では返さない。
--
-- 写すのは、トップレベルの値のうち、数・真偽値・短い文字列（64文字以下）・短い文字列だけの配列。
-- 絞り込める型（選択肢 40文字以下、参照のUUID、日付、日時、数、真偽値）の値は、すべてこれに当てはまる。
-- 本文やグループ（オブジェクト）、長い文字列は写さない。
CREATE FUNCTION list_values_of(fields jsonb) RETURNS jsonb
LANGUAGE sql IMMUTABLE PARALLEL SAFE
SET search_path = pg_catalog
AS $$
    SELECT jsonb_object_agg(f.key, f.value)
    FROM jsonb_each(fields) AS f (key, value)
    WHERE jsonb_typeof(f.value) IN ('number', 'boolean')
       OR (jsonb_typeof(f.value) = 'string' AND char_length(f.value #>> '{}') <= 64)
       OR (jsonb_typeof(f.value) = 'array' AND NOT EXISTS (
            SELECT 1 FROM jsonb_array_elements(f.value) AS x (item)
            WHERE jsonb_typeof(x.item) <> 'string' OR char_length(x.item #>> '{}') > 64))
$$;

ALTER TABLE versions ADD COLUMN list_values jsonb GENERATED ALWAYS AS (list_values_of(fields)) STORED;
