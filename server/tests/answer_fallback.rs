//! 質問応答の根拠の、既定のロケールへのフォールバック（09-qa-chatbot.md、M4）の結合テスト。
//! 断片の選び方（`search::query::fragments`）を、キーワード検索だけで確かめる（LLMの接続先は設定しない）。
//!
//! 環境変数 `YOMI_TEST_PG_BIGM_URL` に、pg_bigm の入った PostgreSQL（`deploy/postgres.Dockerfile`）の
//! スーパーユーザーの接続先があるときだけ実行する。

mod common;

use common::content::*;
use common::server::*;
use common::with_scratch_at;
use serde_json::{Value, json};
use yomi_server::schema::manifest::Manifest;
use yomi_server::search::query::{Services, fragments};
use yomi_server::{worker, workspace_db};

fn parse(res: &Resp) -> Value {
    serde_json::from_str(&res.body).unwrap_or_else(|_| panic!("{}", res.body))
}

#[tokio::test]
async fn answer_fragments_fall_back_to_the_default_locale() {
    with_scratch_at("YOMI_TEST_PG_BIGM_URL", &["pg_bigm"], |app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        // 既定のロケールは ja
        let manifest_json = blog_manifest().replace(
            r#"{"name": "body", "type": "richtext"}"#,
            r#"{"name": "body", "type": "richtext", "searchable": true}"#,
        );
        let res = deploy(&server, &token, "blog", &manifest_json, "").await;
        assert_eq!(res.status, 200, "{}", res.body);
        let manifest: Manifest = serde_json::from_str(&manifest_json).unwrap();
        assert_eq!(manifest.default_locale, "ja");
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        let entries = format!("/api/admin/workspaces/{ws}/entries");

        let publish = |id: String| {
            let (server, root, entries) = (&server, &root, &entries);
            async move {
                let res = send(server, "POST", &format!("{entries}/{id}/publish"), root, Some("{\"revision\":1}")).await;
                assert_eq!(res.status, 200, "{}", res.body);
            }
        };
        let create = |slug: &'static str| {
            let (server, root, entries) = (&server, &root, &entries);
            async move {
                let body = json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": format!("検索語 の記事 {slug}")}]}]});
                let res = send(server, "POST", entries, root, Some(&json!({"contentType": "post", "locale": "ja", "fields": {"title": slug, "slug": slug, "body": body}}).to_string())).await;
                assert_eq!(res.status, 201, "{}", res.body);
                let id = parse(&res)["id"].as_str().unwrap().to_owned();
                publish(id.clone()).await;
                id
            }
        };
        let translate = |id: &str| {
            let (server, root, path) = (&server, &root, format!("{entries}/{id}/translations"));
            async move {
                let res = send(server, "POST", &path, root, Some(&json!({"locale": "en"}).to_string())).await;
                assert_eq!(res.status, 201, "{}", res.body);
                parse(&res)["id"].as_str().unwrap().to_owned()
            }
        };

        // 翻訳のない記事
        let only_ja = create("only-ja").await;
        // 英語の翻訳が公開中の記事（英語の断片だけを使う）
        let translated = create("translated").await;
        let translated_en = translate(&translated).await;
        publish(translated_en.clone()).await;
        // 英語の翻訳が下書きだけの記事（公開中の翻訳はないので、日本語に戻す）
        let draft = create("draft").await;
        translate(&draft).await;
        // 英語の翻訳が公開中でも、チャットボットに使わない設定の記事（見せてよい翻訳はないので、日本語に戻す）
        let excluded = create("excluded").await;
        let excluded_en = translate(&excluded).await;
        publish(excluded_en.clone()).await;
        let res = send(&server, "PATCH", &format!("{entries}/{excluded_en}/publish-settings"), &root, Some(&json!({"useForChatbot": false}).to_string())).await;
        assert_eq!(res.status, 200, "{}", res.body);
        // チャンクを作る
        while worker::run_once(&server.state).await.unwrap() {}

        let state = &server.state;
        let services = Services {
            pool: &state.pool,
            config: &state.config,
            embedding: state.config.llm.as_ref(),
            egress: state.egress.as_ref(),
            cache: &state.search_cache,
        };
        let db = workspace_db::lookup(&state.pool, ws).await.unwrap().unwrap();
        let pick = |locale: &'static str| {
            let (services, db, manifest) = (&services, &db, &manifest);
            async move {
                let mut tx = workspace_db::begin_owned(services.pool, db).await.unwrap();
                let found = fragments(services, &mut tx, manifest, "検索語", locale, 20, 0.2, None).await.unwrap();
                tx.commit().await.unwrap();
                let mut out: Vec<(String, String)> = found.into_iter().map(|f| (f.entry_id.to_string(), f.locale)).collect();
                out.sort();
                out
            }
        };
        let sorted = |mut v: Vec<(String, &str)>| {
            v.sort();
            v.into_iter().map(|(id, l)| (id, l.to_owned())).collect::<Vec<_>>()
        };

        // 英語の質問: 英語の断片と、見せてよい英語の翻訳がない記事の日本語の断片
        assert_eq!(
            pick("en").await,
            sorted(vec![
                (only_ja.clone(), "ja"),
                (translated_en.clone(), "en"),
                (draft.clone(), "ja"),
                (excluded.clone(), "ja"),
            ])
        );
        // 既定のロケールの質問: 日本語の断片だけ（ほかの言語には広げない）
        assert_eq!(
            pick("ja").await,
            sorted(vec![
                (only_ja.clone(), "ja"),
                (translated.clone(), "ja"),
                (draft.clone(), "ja"),
                (excluded.clone(), "ja"),
            ])
        );
        // 翻訳を非公開にすると、次の質問から日本語に戻す（使うたびに今の状態と照合する）
        let res = send(&server, "POST", &format!("{entries}/{translated_en}/unpublish"), &root, None).await;
        assert_eq!(res.status, 204, "{}", res.body);
        let en = pick("en").await;
        assert!(en.contains(&(translated.clone(), "ja".to_owned())), "{en:?}");
        assert!(!en.iter().any(|(id, _)| *id == translated_en), "{en:?}");
    })
    .await;
}
