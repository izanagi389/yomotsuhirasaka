-- まとめて1回だけ動かすジョブ（保存後のフック。保存が止まってから記事ごとに1回。04-hooks.md）。
-- まだ一度も取り出されていないジョブ（attempts = 0）の中で、同じ dedupe_key は1つだけにする。
-- 同じキーのジョブを積み直すと、新しく積まずに run_at だけを延ばす。取り出された後は索引から外れるので、
-- 実行中やり直し待ちのジョブとは別に、次の1回を積める。

ALTER TABLE yomi.jobs ADD COLUMN dedupe_key text;
CREATE UNIQUE INDEX jobs_dedupe_pending ON yomi.jobs (dedupe_key)
    WHERE dedupe_key IS NOT NULL AND attempts = 0 AND failed_at IS NULL;
