//! 記事と下書きの結合テスト（本物のPostgreSQLとHTTPを使う）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::content::*;
use common::server::*;
use common::with_scratch;
use serde_json::{Value, json};
use uuid::Uuid;

fn entry_body(fields: Value) -> String {
    json!({"contentType": "post", "fields": fields}).to_string()
}

fn para(text: &str) -> Value {
    json!({"type": "paragraph", "content": [{"type": "text", "text": text}]})
}

fn doc(nodes: Vec<Value>) -> Value {
    json!({"type": "doc", "content": nodes})
}

fn parse(res: &Resp) -> Value {
    serde_json::from_str(&res.body).unwrap_or_else(|_| panic!("{}", res.body))
}

#[tokio::test]
async fn creating_reading_and_saving_drafts() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        // テンプレートをまだデプロイしていないワークスペース
        let none = workspace_with_template(&server, &root, "none", "blog").await;
        let res = send(&server, "POST", &format!("/api/admin/workspaces/{none}/entries"), &root, Some(&entry_body(json!({})))).await;
        assert_eq!((res.status, res.json_str("error").as_deref()), (409, Some("schema_not_deployed")), "{}", res.body);
        assert_eq!(deploy(&server, &token, "blog", &blog_manifest(), "").await.status, 200);
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        let other_ws = workspace_with_template(&server, &root, "B", "blog").await;
        for (email, role) in [("editor@example.test", "editor"), ("author@example.test", "author"), ("viewer@example.test", "viewer")] {
            create_user(&server.pool, email, false).await;
            add_member(&server, ws, email, role).await;
        }
        create_user(&server.pool, "outsider@example.test", false).await;
        add_member(&server, other_ws, "outsider@example.test", "admin").await;
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        let author = login(&server, "author@example.test", PASSWORD).await;
        let viewer = login(&server, "viewer@example.test", PASSWORD).await;
        let outsider = login(&server, "outsider@example.test", PASSWORD).await;
        let entries = format!("/api/admin/workspaces/{ws}/entries");

        // 作成: 執筆者はできる。閲覧者・他のワークスペースの人はできない
        let body = doc(vec![
            json!({"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "はじめに"}]}),
            para("本文"),
            json!({"type": "heading", "attrs": {"level": 2, "id": "dup"}, "content": [{"type": "text", "text": "a"}]}),
            json!({"type": "heading", "attrs": {"level": 2, "id": "dup"}, "content": [{"type": "text", "text": "b"}]}),
        ]);
        let first = entry_body(json!({"title": "最初の記事", "slug": "first", "body": body}));
        assert_eq!(send(&server, "POST", &entries, &viewer, Some(&first)).await.status, 403);
        assert_eq!(send(&server, "POST", &entries, &outsider, Some(&first)).await.status, 404);
        let res = send(&server, "POST", &entries, &author, Some(&first)).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let created = parse(&res);
        let id = created["id"].as_str().unwrap().to_owned();
        let group = created["translationGroupId"].as_str().unwrap().to_owned();
        assert_eq!(created["locale"], "ja");
        assert_eq!(created["revision"], 1);
        assert_eq!(created["fields"]["title"], "最初の記事");
        assert_eq!(created["fields"]["subtitle"], Value::Null);
        // 見出しのアンカーID: 付与され、重複は2つ目以降だけ直される
        let ids: Vec<&str> = created["fields"]["body"]["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| n["type"] == "heading")
            .map(|n| n["attrs"]["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids.len(), 3);
        assert_eq!(ids[1], "dup");
        assert!(ids[0] != "dup" && ids[2] != "dup" && ids[0] != ids[2]);

        // 読む: 閲覧者も下書きを読める。他のワークスペースの人は読めない
        let path = format!("{entries}/{id}");
        assert_eq!(get(&server, &path, &viewer).await.status, 200);
        assert_eq!(get(&server, &path, &outsider).await.status, 404);
        assert_eq!(get(&server, &format!("/api/admin/workspaces/{other_ws}/entries/{id}"), &outsider).await.status, 404);
        let list = get(&server, &entries, &viewer).await;
        assert!(list.body.contains(&id) && list.body.contains("最初の記事") && !list.body.contains("本文"), "{}", list.body);

        // 作成の検査
        for (bad, status) in [
            (json!({"contentType": "page", "fields": {}}).to_string(), 422),
            (json!({"contentType": "post", "locale": "fr", "fields": {}}).to_string(), 422),
            (entry_body(json!({"slug": "Bad Slug"})), 422),
            (entry_body(json!({"nope": "x"})), 422),
            (entry_body(json!({"body": doc(vec![json!({"type": "script"})])})), 422),
            (entry_body(json!({"body": doc(vec![json!({"type": "paragraph", "content": [{"type": "text", "text": "x", "marks": [{"type": "link", "attrs": {"href": "javascript:alert(1)"}}]}]})])})), 422),
            (entry_body(json!({"body": doc(vec![json!({"type": "embed", "attrs": {"src": "https://evil.example/embed"}})])})), 422),
            (entry_body(json!({"related": [Uuid::new_v4().to_string()]})), 422),
            (entry_body(json!({"slug": "first"})), 409),
            ("{\"contentType\":\"post\",\"fields\":{},\"sql\":1}".to_owned(), 400),
        ] {
            let res = send(&server, "POST", &entries, &editor, Some(&bad)).await;
            assert_eq!(res.status, status, "{bad}: {}", res.body);
        }
        // 別のロケールなら同じスラッグでもよい。参照は存在する翻訳グループならよい
        let res = send(&server, "POST", &entries, &editor, Some(&json!({"contentType": "post", "locale": "en", "fields": {"slug": "first", "related": [group]}}).to_string())).await;
        assert_eq!(res.status, 201, "{}", res.body);

        // 保存: 編集者は執筆者の記事も保存できる。古いリビジョン番号では保存できない（INV-EDIT-01）
        let save = |rev: i64, fields: Value| json!({"revision": rev, "fields": fields}).to_string();
        let draft_path = format!("{path}/draft");
        let res = send(&server, "PUT", &draft_path, &editor, Some(&save(1, json!({"subtitle": "副題"})))).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let saved = parse(&res);
        assert_eq!(saved["revision"], 2);
        assert_eq!(saved["fields"]["subtitle"], "副題");
        assert_eq!(saved["fields"]["title"], "最初の記事", "fields not sent are kept");
        let res = send(&server, "PUT", &draft_path, &author, Some(&save(1, json!({"subtitle": "上書き"})))).await;
        assert_eq!((res.status, res.json_str("error").as_deref()), (409, Some("revision_conflict")));
        assert_eq!(parse(&get(&server, &path, &editor).await)["fields"]["subtitle"], "副題");
        // 不正な本文の保存は拒否され、リビジョン番号も変わらない
        let res = send(&server, "PUT", &draft_path, &editor, Some(&save(2, json!({"body": doc(vec![json!({"type": "text", "text": "x"})])})))).await;
        assert_eq!(res.status, 422);
        assert_eq!(parse(&get(&server, &path, &editor).await)["revision"], 2);
        assert_eq!(send(&server, "PUT", &draft_path, &viewer, Some(&save(2, json!({"subtitle": "x"})))).await.status, 403);
        // アンカーIDは再保存で変わらない
        let current = parse(&get(&server, &path, &editor).await);
        let res = send(&server, "PUT", &draft_path, &editor, Some(&save(2, json!({"body": current["fields"]["body"]})))).await;
        assert_eq!(parse(&res)["fields"]["body"], current["fields"]["body"]);
        // null で空にする
        let res = send(&server, "PUT", &draft_path, &editor, Some(&save(3, json!({"subtitle": null})))).await;
        assert_eq!(parse(&res)["fields"]["subtitle"], Value::Null);

        // 個人トークン: write の範囲なら作れる。read だけなら作れない
        let make_token = |scopes: &str| {
            let server = &server;
            let editor = &editor;
            let body = format!("{{\"name\":\"t\",\"scopes\":[{scopes}],\"workspaceIds\":[\"{ws}\"]}}");
            async move { send(server, "POST", "/api/admin/me/tokens", editor, Some(&body)).await.json_str("token").unwrap() }
        };
        let rw = make_token("\"read\",\"write\"").await;
        let ro = make_token("\"read\"").await;
        let by_token = |t: String, slug: &'static str| {
            let server = &server;
            let entries = entries.clone();
            async move {
                request(server, "POST", &entries, &[("Authorization", &format!("Bearer {t}"))], Some(&entry_body(json!({"slug": slug})))).await.status
            }
        };
        assert_eq!(by_token(rw, "by-token").await, 201);
        assert_eq!(by_token(ro, "by-token-2").await, 403);

        // 所属していないサーバー管理者の作成は、監査ログに残る（Q122）
        let res = send(&server, "POST", &entries, &root, Some(&entry_body(json!({"slug": "by-root"})))).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM yomi.audit_log WHERE workspace_id = $1 AND action = 'entry.created'")
            .bind(ws)
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert_eq!(n, 1);

        // 移行待ちの間は、作成と保存を断る。読むことはできる（INV-SCH-04）
        sqlx::query("UPDATE yomi.workspaces SET schema_state = 'pending', schema_target_version = 2 WHERE id = $1")
            .bind(ws)
            .execute(&server.state.migration)
            .await
            .unwrap();
        let res = send(&server, "POST", &entries, &editor, Some(&entry_body(json!({"slug": "while-pending"})))).await;
        assert_eq!((res.status, res.json_str("error").as_deref()), (409, Some("schema_migration_pending")));
        let rev = parse(&get(&server, &path, &editor).await)["revision"].as_i64().unwrap();
        let res = send(&server, "PUT", &draft_path, &editor, Some(&save(rev, json!({"subtitle": "x"})))).await;
        assert_eq!((res.status, res.json_str("error").as_deref()), (409, Some("schema_migration_pending")));
        assert_eq!(get(&server, &path, &viewer).await.status, 200);
        assert_eq!(get(&server, &entries, &viewer).await.status, 200);
    })
    .await;
}
