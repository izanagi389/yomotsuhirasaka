//! 回答キャッシュ（`answer::cache`）の照合の結合テスト。LLMの接続先は設定しない（回答の中身は約束のテスト
//! `tests/invariants/answer-cache.test.ts` が擬似LLMで確かめる）。
//!
//! 環境変数 `CMS_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use cms_server::answer::{Source, cache};
use cms_server::schema::manifest::Manifest;
use cms_server::workspace_db;
use common::content::*;
use common::server::*;
use common::with_scratch;
use serde_json::{Value, json};

fn parse(res: &Resp) -> Value {
    serde_json::from_str(&res.body).unwrap_or_else(|_| panic!("{}", res.body))
}

#[tokio::test]
async fn cached_answers_are_checked_against_the_current_entries() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        // 確認期限は1日
        let manifest_json = blog_manifest().replace(
            r#""listLimit": 5,"#,
            r#""listLimit": 5, "review": {"intervalDays": 1},"#,
        );
        assert_ne!(manifest_json, blog_manifest());
        assert_eq!(deploy(&server, &token, "blog", &manifest_json, "").await.status, 200);
        let manifest: Manifest = serde_json::from_str(&manifest_json).unwrap();
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        let entries = format!("/api/admin/workspaces/{ws}/entries");
        let body = json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "本文"}]}]});
        let res = send(&server, "POST", &entries, &root, Some(&json!({"contentType": "post", "fields": {"title": "t", "slug": "s", "body": body}}).to_string())).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let id = parse(&res)["id"].as_str().unwrap().to_owned();
        let res = send(&server, "POST", &format!("{entries}/{id}/publish"), &root, Some("{\"revision\":1}")).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let version = parse(&res)["versionId"].as_str().unwrap().to_owned();

        let db = workspace_db::lookup(&server.pool, ws).await.unwrap().unwrap();
        let source = Source {
            citation: 3,
            entry_id: id.parse().unwrap(),
            locale: "ja".into(),
            url: None,
            anchor: None,
            breadcrumb: Vec::new(),
            version_id: version.parse().unwrap(),
            last_reviewed_at: None,
            review_expired: false,
        };
        let key = b"question".to_vec();
        let (pool, db, key, manifest, source) = (&server.pool, &db, &key, &manifest, &source);
        let entry = uuid::Uuid::parse_str(&id).unwrap();
        let store = || async move {
            let mut tx = workspace_db::begin(pool, db).await.unwrap();
            cache::store(&mut tx, key, "答え [1]", false, std::slice::from_ref(source)).await.unwrap();
            tx.commit().await.unwrap();
        };
        let lookup = |locale: &'static str| async move {
            let mut tx = workspace_db::begin(pool, db).await.unwrap();
            let hit = cache::lookup(&mut tx, manifest, key, locale).await.unwrap();
            tx.commit().await.unwrap();
            hit
        };
        let sql = |q: &'static str| async move {
            let mut tx = workspace_db::begin(pool, db).await.unwrap();
            sqlx::query(q).bind(entry).execute(&mut *tx).await.unwrap();
            tx.commit().await.unwrap();
        };

        // 出典が今も見せてよければ返る。URLと鮮度は今の状態から引く
        store().await;
        let hit = lookup("ja").await.expect("cached answer");
        assert_eq!(hit.text, "答え [1]");
        assert_eq!(hit.sources[0].url.as_deref(), Some("https://site.example.test/ja/blog/s"));
        assert!(!hit.sources[0].review_expired);
        // 本文の引用記号の番号も、保存したとおりに返る
        assert_eq!(hit.sources[0].citation, 3);

        // 確認期限が、キャッシュを作った後に来た: 捨てる
        sql("UPDATE entries SET last_verified_at = now() - interval '1 day' + interval '2 seconds' WHERE id = $1").await;
        assert!(lookup("ja").await.is_some(), "期限の前は返る");
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        assert!(lookup("ja").await.is_none(), "確認期限をまたいだキャッシュが返った");
        // 期限切れの後に作ったものは返り、今の時刻で期限切れと判定される
        store().await;
        let hit = lookup("ja").await.expect("cached after expiry");
        assert!(hit.sources[0].review_expired);

        // チャットボットのスイッチを切った: 捨てる
        let res = send(&server, "PATCH", &format!("{entries}/{id}/publish-settings"), &root, Some(&json!({"useForChatbot": false}).to_string())).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert!(lookup("ja").await.is_none());
        let res = send(&server, "PATCH", &format!("{entries}/{id}/publish-settings"), &root, Some(&json!({"useForChatbot": true}).to_string())).await;
        assert_eq!(res.status, 200, "{}", res.body);

        // 既定のロケールへのフォールバックで使った出典（英語の質問に日本語の記事）: 英語の翻訳を公開したら捨てる
        store().await;
        assert!(lookup("en").await.is_some());
        let res = send(&server, "POST", &format!("{entries}/{id}/translations"), &root, Some("{\"locale\":\"en\"}")).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let en = parse(&res)["id"].as_str().unwrap().to_owned();
        let res = send(&server, "POST", &format!("{entries}/{en}/publish"), &root, Some("{\"revision\":1}")).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert!(lookup("en").await.is_none());

        // 非公開にした: 捨てる
        store().await;
        assert!(lookup("ja").await.is_some());
        let res = send(&server, "POST", &format!("{entries}/{id}/unpublish"), &root, None).await;
        assert_eq!(res.status, 204, "{}", res.body);
        assert!(lookup("ja").await.is_none());
    })
    .await;
}
