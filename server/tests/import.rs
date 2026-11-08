//! 取り込みのAPIの結合テスト（本物のPostgreSQLとHTTPを使う）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::content::*;
use common::server::*;
use common::with_scratch;
use serde_json::{Value, json};

fn parse(res: &Resp) -> Value {
    serde_json::from_str(&res.body).unwrap_or_else(|_| panic!("{}", res.body))
}

fn item(key: &str, locale: &str, slug: &str, extra: Value) -> Value {
    let mut v = json!({
        "key": key,
        "locale": locale,
        "originalUrl": format!("https://old.example.test/{locale}/{slug}.html"),
        "fields": {"title": key, "slug": slug,
                   "body": {"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "本文"}]}]}}
    });
    if let (Value::Object(m), Value::Object(e)) = (&mut v, extra) {
        m.extend(e);
    }
    v
}

#[tokio::test]
async fn importing_is_all_or_nothing_and_records_urls_and_sources() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(deploy(&server, &token, "blog", &blog_manifest(), "").await.status, 200);
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        for (email, role) in [("editor@example.test", "editor"), ("author@example.test", "author")] {
            create_user(&server.pool, email, false).await;
            add_member(&server, ws, email, role).await;
        }
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        let author = login(&server, "author@example.test", PASSWORD).await;
        let imports = format!("/api/admin/workspaces/{ws}/imports");
        let entries = format!("/api/admin/workspaces/{ws}/entries");
        let count = || async { parse(&get(&server, &entries, &editor).await).as_array().unwrap().len() };

        let good = vec![
            item("a.md", "ja", "a", json!({"translationKey": "g1"})),
            item("a.en.md", "en", "a", json!({"translationKey": "g1"})),
            item("b.en.md", "en", "b", json!({"translationKey": "g2"})),
        ];
        // 公開できない人は使えない
        let body = json!({"entries": good}).to_string();
        assert_eq!(send(&server, "POST", &imports, &author, Some(&body)).await.status, 403);

        // 1件でも受け付けられなければ、何も書かない
        let mut bad = good.clone();
        bad.push(item("c.md", "de", "c", json!({})));
        let res = send(&server, "POST", &imports, &editor, Some(&json!({"entries": bad}).to_string())).await;
        assert_eq!(res.status, 422, "{}", res.body);
        assert!(res.body.contains("c.md: locale"), "{}", res.body);
        let mut dup = good.clone();
        dup.push(item("a2.md", "ja", "a", json!({"originalUrl": "https://old.example.test/other"})));
        let res = send(&server, "POST", &imports, &editor, Some(&json!({"entries": dup}).to_string())).await;
        assert_eq!(res.status, 409, "{}", res.body);
        assert_eq!(count().await, 0);
        for (label, entries) in [
            ("same locale twice in a group", vec![item("x.md", "ja", "x", json!({"translationKey": "g"})), item("y.md", "ja", "y", json!({"translationKey": "g"}))]),
            ("translationOf without key", vec![item("x.md", "ja", "x", json!({"translationOf": "y"}))]),
            ("translationOf outside the group", vec![item("x.md", "ja", "x", json!({"translationKey": "g", "translationOf": "zzz"}))]),
            ("relative original URL", vec![item("x.md", "ja", "x", json!({"originalUrl": "/old/x"}))]),
            ("no slug", vec![json!({"key": "x.md", "locale": "ja", "originalUrl": "https://old.example.test/x", "fields": {"title": "t"}})]),
            ("reference field", vec![item("x.md", "ja", "x", json!({"fields": {"title": "t", "slug": "x", "related": []}}))]),
            ("image without alt", vec![item("x.md", "ja", "x", json!({"fields": {"title": "t", "slug": "x", "body": {"type": "doc", "content": [{"type": "image", "attrs": {"mediaId": uuid::Uuid::new_v4().to_string()}}]}}}))]),
        ] {
            let res = send(&server, "POST", &imports, &editor, Some(&json!({"entries": entries}).to_string())).await;
            assert!(matches!(res.status, 409 | 422), "{label}: {} {}", res.status, res.body);
        }
        assert_eq!(count().await, 0);

        // 取り込む: 公開し、元のURLを変更履歴に残し、翻訳の元を記録する
        let res = send(&server, "POST", &imports, &editor, Some(&body)).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let out = parse(&res)["entries"].as_array().unwrap().clone();
        assert_eq!(out.len(), 3);
        let id = |key: &str| out.iter().find(|e| e["key"] == key).unwrap()["id"].as_str().unwrap().to_owned();
        let version = |key: &str| out.iter().find(|e| e["key"] == key).unwrap()["versionId"].as_str().unwrap().to_owned();
        let history = parse(&get(&server, &format!("{entries}/{}/url-history", id("a.en.md")), &editor).await);
        assert_eq!(history[0]["from"], "https://old.example.test/en/a.html");
        assert_eq!(history[0]["to"], "https://site.example.test/en/blog/a");
        let ja = parse(&get(&server, &format!("{entries}/{}/translation", id("a.md")), &editor).await);
        let en = parse(&get(&server, &format!("{entries}/{}/translation", id("a.en.md")), &editor).await);
        assert_eq!(ja["groupId"], en["groupId"]);
        assert!(ja["sourceEntryId"].is_null());
        assert_eq!(en["sourceEntryId"], id("a.md"));
        assert_eq!(en["reflectedSourceVersionId"], version("a.md"));
        // 既定のロケールの記事がないグループは「元なし」
        let b = parse(&get(&server, &format!("{entries}/{}/translation", id("b.en.md")), &editor).await);
        assert!(b["sourceEntryId"].is_null());
        assert_ne!(b["groupId"], ja["groupId"]);
        let audited: i64 = sqlx::query_scalar("SELECT count(*) FROM yomi.audit_log WHERE workspace_id = $1 AND action = 'entry.imported'")
            .bind(ws)
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert_eq!(audited, 1);
    })
    .await;
}
