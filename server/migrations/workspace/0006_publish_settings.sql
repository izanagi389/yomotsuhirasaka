-- 記事の公開設定のスイッチ（03-content-and-editing.md「管理画面の体験」、07-ai-delivery.md、Q184）。
-- M1では保存と変更のWebhookだけ。スイッチの効果（AI向けの配信、学習への利用の拒否のメタタグなど）はM2以降。
-- 版には含めない（公開し直さずに切り替える記事の設定）。
ALTER TABLE entries
    -- チャットボットの回答の根拠に使う（M4）
    ADD COLUMN use_for_chatbot boolean NOT NULL DEFAULT true,
    -- 外部AI向けの素材（Markdown版・目録）に含める（M2、Q63）
    ADD COLUMN include_in_ai_materials boolean NOT NULL DEFAULT true,
    -- 学習への利用を拒否する（M2、Q35）
    ADD COLUMN opt_out_of_ai_training boolean NOT NULL DEFAULT false;
