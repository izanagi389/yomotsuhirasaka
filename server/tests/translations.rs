//! 翻訳の作成と、元の記事・版の記録の結合テスト（本物のPostgreSQLとHTTPを使う）。
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

fn error(res: &Resp) -> (u16, Option<String>) {
    (res.status, res.json_str("error"))
}

fn body(text: &str) -> Value {
    json!({"type": "doc", "content": [
        {"type": "heading", "attrs": {"level": 2, "id": "intro"}, "content": [{"type": "text", "text": "見出し"}]},
        {"type": "paragraph", "content": [{"type": "text", "text": text}]}
    ]})
}

#[tokio::test]
async fn creating_translations_and_recording_their_source() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(deploy(&server, &token, "blog", &blog_manifest(), "").await.status, 200);
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        for (email, role) in [("editor@example.test", "editor"), ("author@example.test", "author"), ("viewer@example.test", "viewer")] {
            create_user(&server.pool, email, false).await;
            add_member(&server, ws, email, role).await;
        }
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        let author = login(&server, "author@example.test", PASSWORD).await;
        let viewer = login(&server, "viewer@example.test", PASSWORD).await;
        let entries = format!("/api/admin/workspaces/{ws}/entries");
        let create = |locale: &str, slug: &str, text: &str| json!({"contentType": "post", "locale": locale, "fields": {"title": text, "slug": slug, "body": body(text)}}).to_string();
        let new_entry = |locale: &'static str, slug: &'static str, text: &'static str| {
            let (server, editor, entries) = (&server, &editor, entries.clone());
            async move {
                let res = send(server, "POST", &entries, editor, Some(&create(locale, slug, text))).await;
                assert_eq!(res.status, 201, "{}", res.body);
                parse(&res)["id"].as_str().unwrap().to_owned()
            }
        };
        let translate = |id: &str, locale: &str, s: &Login| {
            let (server, path, body) = (&server, format!("{entries}/{id}/translations"), json!({"locale": locale}).to_string());
            let s = s.clone();
            async move { send(server, "POST", &path, &s, Some(&body)).await }
        };
        let info = |id: &str| {
            let (server, path, viewer) = (&server, format!("{entries}/{id}/translation"), &viewer);
            async move { parse(&get(server, &path, viewer).await) }
        };

        // 公開済みの日本語の記事
        let ja = new_entry("ja", "hello", "日本語の本文").await;
        let res = send(&server, "POST", &format!("{entries}/{ja}/publish"), &editor, Some(&json!({"revision": 1}).to_string())).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let ja_v1 = parse(&res)["versionId"].as_str().unwrap().to_owned();

        // 作成の権限と検査
        assert_eq!(translate(&ja, "en", &viewer).await.status, 403);
        assert_eq!(error(&translate(&ja, "fr", &editor).await), (422, Some("invalid_entry".into())));
        assert_eq!(translate(&uuid::Uuid::new_v4().to_string(), "en", &editor).await.status, 404);
        assert_eq!(error(&translate(&ja, "ja", &editor).await), (409, Some("translation_exists".into())));

        // 執筆者も翻訳を作れる。内容は複製され、同じ翻訳グループに入る。元の記事と、その時点の公開中の版が記録される
        let res = translate(&ja, "en", &author).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let en_draft = parse(&res);
        let en = en_draft["id"].as_str().unwrap().to_owned();
        assert_eq!(en_draft["locale"], "en");
        assert_eq!(en_draft["revision"], 1);
        assert_eq!(en_draft["publishedVersionId"], Value::Null, "a translation starts as a draft");
        let ja_draft = parse(&get(&server, &format!("{entries}/{ja}"), &viewer).await);
        assert_eq!(en_draft["translationGroupId"], ja_draft["translationGroupId"]);
        assert_eq!(en_draft["fields"], ja_draft["fields"], "fields (including anchor IDs and slug) are copied");
        let en_info = info(&en).await;
        assert_eq!(en_info["groupId"], ja_draft["translationGroupId"]);
        assert_eq!((en_info["sourceEntryId"].as_str(), en_info["sourceLocale"].as_str()), (Some(&*ja), Some("ja")));
        assert_eq!(en_info["reflectedSourceVersionId"], ja_v1.as_str());
        let ja_info = info(&ja).await;
        assert_eq!((ja_info["sourceEntryId"].clone(), ja_info["reflectedSourceVersionId"].clone()), (Value::Null, Value::Null), "the original has no source");
        let members: Vec<(String, bool)> = ja_info["members"].as_array().unwrap().iter().map(|m| (m["locale"].as_str().unwrap().to_owned(), m["published"].as_bool().unwrap())).collect();
        assert_eq!(members, [("en".to_owned(), false), ("ja".to_owned(), true)]);
        // 同じロケールの翻訳は2つ作れない（翻訳から作っても同じ）
        assert_eq!(error(&translate(&ja, "en", &editor).await), (409, Some("translation_exists".into())));
        assert_eq!(error(&translate(&en, "ja", &editor).await), (409, Some("translation_exists".into())));

        // 翻訳を編集しても、元の記事は変わらない
        let res = send(&server, "PUT", &format!("{entries}/{en}/draft"), &editor, Some(&json!({"revision": 1, "fields": {"title": "English"}}).to_string())).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert_eq!(parse(&get(&server, &format!("{entries}/{ja}"), &viewer).await), ja_draft);

        // 元の記事が公開されていなければ、反映済みの版は記録されない
        let draft_only = new_entry("ja", "draft-only", "下書きだけ").await;
        let res = translate(&draft_only, "en", &editor).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let t = parse(&res)["id"].as_str().unwrap().to_owned();
        assert_eq!(info(&t).await["sourceEntryId"], draft_only.as_str());
        assert_eq!(info(&t).await["reflectedSourceVersionId"], Value::Null);

        // 新しいロケールで他の記事がスラッグを使っていれば、スラッグは空にする
        new_entry("en", "taken", "English").await;
        let ja_taken = new_entry("ja", "taken", "日本語").await;
        let res = translate(&ja_taken, "en", &editor).await;
        assert_eq!(res.status, 201, "{}", res.body);
        assert_eq!(parse(&res)["fields"]["slug"], Value::Null);
        assert_eq!(parse(&res)["fields"]["title"], "日本語");

        // 元の記事を削除すると「元なし」になる。そのロケールの翻訳を作り直せる
        assert_eq!(send(&server, "DELETE", &format!("{entries}/{ja}"), &editor, None).await.status, 204);
        let en_info = info(&en).await;
        assert_eq!((en_info["sourceEntryId"].clone(), en_info["reflectedSourceVersionId"].clone()), (Value::Null, Value::Null));
        assert_eq!(en_info["members"].as_array().unwrap().len(), 1);
        assert_eq!(get(&server, &format!("{entries}/{ja}/translation"), &viewer).await.status, 404);
        let res = translate(&en, "ja", &editor).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let ja2 = parse(&res)["id"].as_str().unwrap().to_owned();
        assert_eq!(info(&ja2).await["sourceEntryId"], en.as_str());
        assert_eq!(info(&ja2).await["reflectedSourceVersionId"], Value::Null, "en has not been published");
        assert_eq!(parse(&get(&server, &format!("{entries}/{ja2}"), &viewer).await)["fields"]["title"], "English");

        // 移行待ちの間は作れない。読むことはできる
        sqlx::query("UPDATE yomi.workspaces SET schema_state = 'pending', schema_target_version = 2 WHERE id = $1")
            .bind(ws)
            .execute(&server.state.migration)
            .await
            .unwrap();
        assert_eq!(error(&translate(&ja_taken, "en", &editor).await).1.as_deref(), Some("schema_migration_pending"));
        assert_eq!(get(&server, &format!("{entries}/{en}/translation"), &viewer).await.status, 200);
    })
    .await;
}
