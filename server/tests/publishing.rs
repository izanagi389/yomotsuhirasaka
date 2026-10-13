//! 公開・非公開・削除、版、URLの変更履歴の結合テスト（本物のPostgreSQLとHTTPを使う）。
//!
//! 環境変数 `CMS_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::content::*;
use common::server::*;
use common::with_scratch;
use serde_json::{Value, json};
use uuid::Uuid;

const URL_BASE: &str = "https://site.example.test/ja/blog";

fn para(text: &str) -> Value {
    json!({"type": "paragraph", "content": [{"type": "text", "text": text}]})
}

fn heading(text: &str, id: &str) -> Value {
    json!({"type": "heading", "attrs": {"level": 2, "id": id}, "content": [{"type": "text", "text": text}]})
}

fn image(attrs: Value) -> Value {
    json!({"type": "image", "attrs": attrs})
}

fn doc(nodes: Vec<Value>) -> Value {
    json!({"type": "doc", "content": nodes})
}

fn parse(res: &Resp) -> Value {
    serde_json::from_str(&res.body).unwrap_or_else(|_| panic!("{}", res.body))
}

fn error(res: &Resp) -> (u16, Option<String>) {
    (res.status, res.json_str("error"))
}

async fn revision(server: &Server, path: &str, s: &Login) -> i64 {
    parse(&get(server, path, s).await)["revision"]
        .as_i64()
        .unwrap()
}

async fn audit_count(server: &Server, ws: Uuid, action: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM cms.audit_log WHERE workspace_id = $1 AND action = $2")
        .bind(ws)
        .bind(action)
        .fetch_one(&server.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn publishing_versions_and_url_history() {
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

        // 執筆者が書いた記事。画像に代替テキストがない
        let body = doc(vec![
            para("導入"),
            heading("A", "sec-a"),
            para("Aの本文"),
            heading("B", "sec-b"),
            para("Bの本文"),
            image(json!({"mediaId": Uuid::new_v4().to_string()})),
        ]);
        let create = json!({"contentType": "post", "fields": {"title": "記事", "slug": "first", "body": body}}).to_string();
        let res = send(&server, "POST", &entries, &author, Some(&create)).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let id = parse(&res)["id"].as_str().unwrap().to_owned();
        let path = format!("{entries}/{id}");
        let publish = |rev: i64, correction: bool| json!({"revision": rev, "correction": correction}).to_string();
        let publish_path = format!("{path}/publish");

        // 執筆者・閲覧者は公開できない（INV-PERM-01）
        for s in [&author, &viewer] {
            assert_eq!(send(&server, "POST", &publish_path, s, Some(&publish(1, false))).await.status, 403);
            assert_eq!(send(&server, "POST", &publish_path, s, Some(&publish(1, true))).await.status, 403);
        }
        // 一度も公開していない記事は、訂正として公開できない
        let res = send(&server, "POST", &publish_path, &editor, Some(&publish(1, true))).await;
        assert_eq!(error(&res), (409, Some("nothing_to_correct".into())), "{}", res.body);
        // 古いリビジョン番号では公開できない
        let res = send(&server, "POST", &publish_path, &editor, Some(&publish(0, false))).await;
        assert_eq!(error(&res), (409, Some("revision_conflict".into())));
        // 代替テキストのない画像があると公開できない（INV-IMG-01）。説明には場所だけが出る
        let res = send(&server, "POST", &publish_path, &editor, Some(&publish(1, false))).await;
        assert_eq!(error(&res), (422, Some("alt_text_missing".into())), "{}", res.body);
        assert!(res.body.contains("fields.body.content[5]") && !res.body.contains("Bの本文"), "{}", res.body);
        // 必須のフィールドが空でも公開できない
        let save = |rev: i64, fields: Value| json!({"revision": rev, "fields": fields}).to_string();
        let draft_path = format!("{path}/draft");
        let fixed = doc(vec![
            para("導入"),
            heading("A", "sec-a"),
            para("Aの本文"),
            heading("B", "sec-b"),
            para("Bの本文"),
            image(json!({"mediaId": Uuid::new_v4().to_string(), "decorative": true})),
        ]);
        let res = send(&server, "PUT", &draft_path, &author, Some(&save(1, json!({"title": null, "body": fixed})))).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let res = send(&server, "POST", &publish_path, &editor, Some(&publish(2, false))).await;
        assert_eq!(error(&res), (422, Some("invalid_entry".into())), "{}", res.body);
        assert!(res.body.contains("fields.title"), "{}", res.body);
        let res = send(&server, "PUT", &draft_path, &author, Some(&save(2, json!({"title": "記事"})))).await;
        assert_eq!(res.status, 200);

        // 公開する
        let res = send(&server, "POST", &publish_path, &editor, Some(&publish(3, false))).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let v1 = parse(&res);
        assert_eq!((v1["number"].as_i64(), v1["url"].as_str()), (Some(1), Some(&*format!("{URL_BASE}/first"))));
        assert_eq!((v1["isCorrection"].as_bool(), v1["previousUrl"].clone()), (Some(false), Value::Null));
        let v1_id = v1["versionId"].as_str().unwrap().to_owned();
        assert_eq!(parse(&get(&server, &path, &viewer).await)["publishedVersionId"], v1_id.as_str());

        // スラッグを変えて公開すると、URLの変更が記録される（INV-URL-01）
        let rev = revision(&server, &path, &editor).await;
        assert_eq!(send(&server, "PUT", &draft_path, &editor, Some(&save(rev, json!({"slug": "second"})))).await.status, 200);
        let res = send(&server, "POST", &publish_path, &editor, Some(&publish(rev + 1, false))).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert_eq!(parse(&res)["previousUrl"], format!("{URL_BASE}/first"));
        // 公開URLを変えずに公開しても、変更は記録されない
        let rev = revision(&server, &path, &editor).await;
        assert_eq!(send(&server, "POST", &publish_path, &editor, Some(&publish(rev, false))).await.status, 200);
        let history = parse(&get(&server, &format!("{path}/url-history"), &viewer).await);
        let pairs: Vec<(&str, &str)> = history.as_array().unwrap().iter().map(|c| (c["from"].as_str().unwrap(), c["to"].as_str().unwrap())).collect();
        assert_eq!(pairs, [(&*format!("{URL_BASE}/first"), &*format!("{URL_BASE}/second"))]);

        // 前の公開URLは、別の記事が下書きでスラッグに使えるが、公開中の記事のURLは使えない
        let other = json!({"contentType": "post", "fields": {"title": "別の記事", "slug": "second-copy", "body": doc(vec![para("x")])}}).to_string();
        let other_id = parse(&send(&server, "POST", &entries, &editor, Some(&other)).await)["id"].as_str().unwrap().to_owned();
        let other_path = format!("{entries}/{other_id}");
        assert_eq!(send(&server, "PUT", &format!("{other_path}/draft"), &editor, Some(&save(1, json!({"slug": "first"})))).await.status, 200);
        assert_eq!(send(&server, "POST", &format!("{other_path}/publish"), &editor, Some(&publish(2, false))).await.status, 200);
        let rev = revision(&server, &path, &editor).await;
        assert_eq!(send(&server, "PUT", &draft_path, &editor, Some(&save(rev, json!({"slug": "third"})))).await.status, 200);
        assert_eq!(send(&server, "PUT", &format!("{other_path}/draft"), &editor, Some(&save(2, json!({"slug": "second"})))).await.status, 200);
        let res = send(&server, "POST", &format!("{other_path}/publish"), &editor, Some(&publish(3, false))).await;
        assert_eq!(error(&res), (409, Some("url_taken".into())), "{}", res.body);

        assert_eq!(send(&server, "PUT", &format!("{other_path}/draft"), &editor, Some(&save(3, json!({"slug": "other"})))).await.status, 200);

        // 訂正として公開: セクションBを直し、セクションAを消す（INV-COR-01）
        let corrected = doc(vec![para("導入"), heading("B", "sec-b"), para("Bの本文（訂正）"), image(json!({"mediaId": Uuid::new_v4().to_string(), "alt": "図"}))]);
        let rev = revision(&server, &path, &editor).await;
        assert_eq!(send(&server, "PUT", &draft_path, &editor, Some(&save(rev, json!({"slug": "second", "body": corrected})))).await.status, 200);
        let res = send(&server, "POST", &publish_path, &editor, Some(&publish(rev + 1, true))).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert_eq!(parse(&res)["isCorrection"], true);
        assert_eq!(audit_count(&server, ws, "entry.published_as_correction").await, 1, "corrections are always audited");
        assert_eq!(audit_count(&server, ws, "entry.published").await, 0, "members' ordinary publishing is not audited");

        // 版の履歴: 新しい順。公開中の版と訂正の印、影響したセクション
        let versions = parse(&get(&server, &format!("{path}/versions"), &viewer).await);
        let versions = versions.as_array().unwrap();
        assert_eq!(versions.iter().map(|v| v["number"].as_i64().unwrap()).collect::<Vec<_>>(), [4, 3, 2, 1]);
        assert_eq!(versions.iter().map(|v| v["current"].as_bool().unwrap()).collect::<Vec<_>>(), [true, false, false, false]);
        assert_eq!(versions.iter().map(|v| v["isCorrection"].as_bool().unwrap()).collect::<Vec<_>>(), [true, false, false, false]);
        assert_eq!(versions[0]["affectedAnchors"], json!(["sec-a", "sec-b"]));
        assert_eq!(versions[0]["affectsLead"], false);
        assert_eq!(versions[0]["anchors"], json!(["sec-b"]));
        assert_eq!(versions[3]["anchors"], json!(["sec-a", "sec-b"]));
        assert_eq!(versions[3]["affectedAnchors"], Value::Null);
        assert!(versions.iter().all(|v| v["erased"] == false));
        // 版の中身は、公開した時点のまま
        let first = parse(&get(&server, &format!("{path}/versions/{v1_id}"), &viewer).await);
        assert_eq!(first["fields"]["slug"], "first");
        assert!(first["fields"]["body"].to_string().contains("Aの本文"));
        assert_eq!(get(&server, &format!("{path}/versions/{}", Uuid::new_v4()), &viewer).await.status, 404);
        assert_eq!(get(&server, &format!("{other_path}/versions/{v1_id}"), &viewer).await.status, 404, "a version of another entry");

        // 非公開: 執筆者はできない。版は残り、もう一度公開できる
        let unpublish_path = format!("{path}/unpublish");
        assert_eq!(send(&server, "POST", &unpublish_path, &author, None).await.status, 403);
        assert_eq!(send(&server, "POST", &unpublish_path, &editor, None).await.status, 204);
        assert_eq!(send(&server, "POST", &unpublish_path, &editor, None).await.status, 204, "unpublishing twice is fine");
        assert_eq!(parse(&get(&server, &path, &viewer).await)["publishedVersionId"], Value::Null);
        let versions = parse(&get(&server, &format!("{path}/versions"), &viewer).await);
        assert_eq!(versions.as_array().unwrap().len(), 4);
        assert!(versions.as_array().unwrap().iter().all(|v| v["current"] == false));
        // 非公開にした記事のURLは、他の記事が公開に使える
        let rev = revision(&server, &path, &editor).await;
        assert_eq!(send(&server, "PUT", &draft_path, &editor, Some(&save(rev, json!({"slug": "fourth"})))).await.status, 200);
        assert_eq!(send(&server, "PUT", &format!("{other_path}/draft"), &editor, Some(&save(4, json!({"slug": "second"})))).await.status, 200);
        let res = send(&server, "POST", &format!("{other_path}/publish"), &editor, Some(&publish(5, false))).await;
        assert_eq!(res.status, 200, "{}", res.body);

        // 削除: 執筆者・閲覧者はできない（Q200）。削除した記事は読めず、スラッグとURLは他の記事が使える
        assert_eq!(send(&server, "DELETE", &other_path, &author, None).await.status, 403);
        assert_eq!(send(&server, "DELETE", &other_path, &viewer, None).await.status, 403);
        assert_eq!(send(&server, "DELETE", &other_path, &editor, None).await.status, 204);
        assert_eq!(send(&server, "DELETE", &other_path, &editor, None).await.status, 404);
        for p in [other_path.clone(), format!("{other_path}/versions"), format!("{other_path}/url-history")] {
            assert_eq!(get(&server, &p, &viewer).await.status, 404, "{p}");
        }
        assert_eq!(send(&server, "POST", &format!("{other_path}/publish"), &editor, Some(&publish(5, false))).await.status, 404);
        assert!(!get(&server, &entries, &viewer).await.body.contains(&other_id));
        let rev = revision(&server, &path, &editor).await;
        assert_eq!(send(&server, "PUT", &draft_path, &editor, Some(&save(rev, json!({"slug": "second"})))).await.status, 200);
        let res = send(&server, "POST", &publish_path, &editor, Some(&publish(rev + 1, false))).await;
        assert_eq!(res.status, 200, "{}", res.body);

        // 所属していないサーバー管理者の公開・非公開・削除は、監査ログに残る（Q122）
        let rev = revision(&server, &path, &root).await;
        assert_eq!(send(&server, "POST", &publish_path, &root, Some(&publish(rev, false))).await.status, 200);
        assert_eq!(send(&server, "POST", &unpublish_path, &root, None).await.status, 204);
        for a in ["entry.published", "entry.unpublished"] {
            assert_eq!(audit_count(&server, ws, a).await, 1, "{a}");
        }
        assert_eq!(audit_count(&server, ws, "entry.deleted").await, 0);

        // 移行待ちの間: 公開は断る。非公開と削除はできる（取り下げを止めないため）
        let rev = revision(&server, &path, &editor).await;
        assert_eq!(send(&server, "POST", &publish_path, &editor, Some(&publish(rev, false))).await.status, 200);
        sqlx::query("UPDATE cms.workspaces SET schema_state = 'pending', schema_target_version = 2 WHERE id = $1")
            .bind(ws)
            .execute(&server.state.migration)
            .await
            .unwrap();
        let res = send(&server, "POST", &publish_path, &editor, Some(&publish(rev, false))).await;
        assert_eq!(error(&res), (409, Some("schema_migration_pending".into())));
        assert_eq!(send(&server, "POST", &unpublish_path, &editor, None).await.status, 204);
        assert_eq!(get(&server, &format!("{path}/versions"), &viewer).await.status, 200);
        assert_eq!(send(&server, "DELETE", &path, &editor, None).await.status, 204);
    })
    .await;
}
