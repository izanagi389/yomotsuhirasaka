//! 管理APIの回数、記事の数、登録クエリの数と回数の上限の結合テスト（13-security.md「ワークスペースごとの上限」）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::content::*;
use common::server::*;
use common::with_scratch;
use serde_json::{Value, json};
use uuid::Uuid;

fn code(res: &Resp) -> (u16, Option<String>) {
    (res.status, res.json_str("error"))
}

fn entry(slug: &str) -> String {
    let body = json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "本文"}]}]});
    json!({"contentType": "post", "fields": {"title": "t", "slug": slug, "body": body}}).to_string()
}

async fn register(server: &Server, ws: Uuid, key: &str, query: &str) -> Resp {
    request(
        server,
        "POST",
        &format!("/ws/{ws}/graphql/queries"),
        &[("Authorization", &format!("Bearer {key}"))],
        Some(&json!({"query": query}).to_string()),
    )
    .await
}

async fn api_key(server: &Server, root: &Login, ws: Uuid) -> String {
    let res = send(
        server,
        "POST",
        &format!("/api/admin/workspaces/{ws}/api-keys"),
        root,
        Some("{\"scopes\":[\"read\",\"register\"]}"),
    )
    .await;
    assert_eq!(res.status, 201, "{}", res.body);
    res.json_str("key").unwrap()
}

#[tokio::test]
async fn admin_requests_are_limited_per_user() {
    with_scratch(|app, migration| async move {
        let mut cfg = config(&app, &migration);
        cfg.limits.admin_requests_per_user_per_minute = 5;
        let server = start(cfg).await;
        for email in ["a@example.test", "b@example.test"] {
            create_user(&server.pool, email, false).await;
        }
        let a = login(&server, "a@example.test", PASSWORD).await;
        let b = login(&server, "b@example.test", PASSWORD).await;
        let mut statuses = Vec::new();
        for _ in 0..8 {
            statuses.push(get(&server, "/api/admin/me", &a).await);
        }
        let last = statuses.last().unwrap();
        assert_eq!(
            code(last),
            (429, Some("admin_rate_limited".to_owned())),
            "{}",
            last.body
        );
        assert!(last.header("retry-after").is_some());
        // 他の人は止まらない。認証を通らないリクエストは、誰の枠も使わない
        assert_eq!(get(&server, "/api/admin/me", &b).await.status, 200);
        assert_eq!(
            request(&server, "GET", "/api/admin/me", &[], None)
                .await
                .status,
            401
        );
    })
    .await;
}

#[tokio::test]
async fn admin_requests_are_limited_per_workspace_counting_members_only() {
    with_scratch(|app, migration| async move {
        let mut cfg = config(&app, &migration);
        cfg.limits.admin_requests_per_workspace_per_minute = 6;
        let server = start(cfg).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(
            deploy(&server, &token, "blog", &blog_manifest(), "")
                .await
                .status,
            200
        );
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        let other = workspace_with_template(&server, &root, "B", "blog").await;
        for (email, target) in [
            ("member@example.test", ws),
            ("outsider@example.test", other),
        ] {
            create_user(&server.pool, email, false).await;
            add_member(&server, target, email, "viewer").await;
        }
        let member = login(&server, "member@example.test", PASSWORD).await;
        let outsider = login(&server, "outsider@example.test", PASSWORD).await;
        let entries = format!("/api/admin/workspaces/{ws}/entries");

        // 所属しない人が何度送っても、そのワークスペースの枠は減らない
        for _ in 0..20 {
            assert_eq!(get(&server, &entries, &outsider).await.status, 404);
        }
        let mut last = get(&server, &entries, &member).await;
        assert_eq!(last.status, 200, "{}", last.body);
        for _ in 0..10 {
            last = get(&server, &entries, &member).await;
        }
        assert_eq!(
            code(&last),
            (429, Some("admin_rate_limited".to_owned())),
            "{}",
            last.body
        );
        // 他のワークスペースは止まらない
        let there = format!("/api/admin/workspaces/{other}/entries");
        assert_eq!(get(&server, &there, &outsider).await.status, 200);
    })
    .await;
}

#[tokio::test]
async fn entries_are_limited_per_workspace() {
    with_scratch(|app, migration| async move {
        let mut cfg = config(&app, &migration);
        cfg.limits.max_entries_per_workspace = 2;
        let server = start(cfg).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(deploy(&server, &token, "blog", &blog_manifest(), "").await.status, 200);
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        let other = workspace_with_template(&server, &root, "B", "blog").await;
        let entries = format!("/api/admin/workspaces/{ws}/entries");
        let imports = format!("/api/admin/workspaces/{ws}/imports");
        let first = send(&server, "POST", &entries, &root, Some(&entry("a"))).await;
        assert_eq!(first.status, 201, "{}", first.body);
        let first: Value = serde_json::from_str(&first.body).unwrap();

        // 取り込みは全件が収まるかを先に見る（2件は上限を超える）
        let item = |slug: &str| {
            json!({"key": format!("{slug}.md"), "locale": "ja", "originalUrl": format!("https://old.example.test/{slug}"), "fields": {"title": "t", "slug": slug, "body": {"type": "doc", "content": []}}})
        };
        let res = send(&server, "POST", &imports, &root, Some(&json!({"entries": [item("b"), item("c")]}).to_string())).await;
        assert_eq!(code(&res), (409, Some("entry_limit_reached".to_owned())), "{}", res.body);

        assert_eq!(send(&server, "POST", &entries, &root, Some(&entry("b"))).await.status, 201);
        let res = send(&server, "POST", &entries, &root, Some(&entry("c"))).await;
        assert_eq!(code(&res), (409, Some("entry_limit_reached".to_owned())), "{}", res.body);
        // 翻訳も1件の記事として数える
        let id = first["id"].as_str().unwrap();
        let res = send(&server, "POST", &format!("{entries}/{id}/translations"), &root, Some("{\"locale\":\"en\"}")).await;
        assert_eq!(code(&res), (409, Some("entry_limit_reached".to_owned())), "{}", res.body);
        // 他のワークスペースは別に数える
        let there = format!("/api/admin/workspaces/{other}/entries");
        assert_eq!(send(&server, "POST", &there, &root, Some(&entry("a"))).await.status, 201);
    })
    .await;
}

#[tokio::test]
async fn registered_queries_are_limited_in_number() {
    with_scratch(|app, migration| async move {
        let mut cfg = config(&app, &migration);
        cfg.limits.max_queries_per_workspace = 2;
        let server = start(cfg).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(
            deploy(&server, &token, "blog", &blog_manifest(), "")
                .await
                .status,
            200
        );
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        let key = api_key(&server, &root, ws).await;
        assert_eq!(
            register(&server, ws, &key, "query A { __typename }")
                .await
                .status,
            201
        );
        assert_eq!(
            register(&server, ws, &key, "query B { __typename }")
                .await
                .status,
            201
        );
        let res = register(&server, ws, &key, "query C { __typename }").await;
        assert_eq!(
            code(&res),
            (409, Some("query_limit_reached".to_owned())),
            "{}",
            res.body
        );
        // 登録済みの文は、上限に達していても登録し直せる（サイトのビルドを止めない）
        assert_eq!(
            register(&server, ws, &key, "query A { __typename }")
                .await
                .status,
            201
        );
    })
    .await;
}

#[tokio::test]
async fn new_query_registrations_are_limited_per_minute() {
    with_scratch(|app, migration| async move {
        let mut cfg = config(&app, &migration);
        cfg.limits.query_registrations_per_workspace_per_minute = 2;
        let server = start(cfg).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(
            deploy(&server, &token, "blog", &blog_manifest(), "")
                .await
                .status,
            200
        );
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        let key = api_key(&server, &root, ws).await;
        assert_eq!(
            register(&server, ws, &key, "query A { __typename }")
                .await
                .status,
            201
        );
        // 不正な文も数える（検証の手間を払わせ続けられないように）
        assert_eq!(
            register(&server, ws, &key, "query B { nothing }")
                .await
                .status,
            422
        );
        let res = register(&server, ws, &key, "query C { __typename }").await;
        assert_eq!(
            code(&res),
            (429, Some("query_registration_rate_limited".to_owned())),
            "{}",
            res.body
        );
        // 登録済みの文の登録し直しは数えない
        assert_eq!(
            register(&server, ws, &key, "query A { __typename }")
                .await
                .status,
            201
        );
    })
    .await;
}

/// 版の数の上限（R14）。記事ごとと、ワークスペースごと。上限に達したら公開を断り、古い版は消さない。
#[tokio::test]
async fn versions_are_limited_per_entry_and_workspace() {
    with_scratch(|app, migration| async move {
        let mut cfg = config(&app, &migration);
        cfg.limits.max_versions_per_entry = 2;
        cfg.limits.max_versions_per_workspace = 3;
        let server = start(cfg).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(
            deploy(&server, &token, "blog", &blog_manifest(), "")
                .await
                .status,
            200
        );
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        let other = workspace_with_template(&server, &root, "B", "blog").await;
        let create = |ws: Uuid, slug: &'static str| {
            let (server, root) = (&server, &root);
            async move {
                let res = send(
                    server,
                    "POST",
                    &format!("/api/admin/workspaces/{ws}/entries"),
                    root,
                    Some(&entry(slug)),
                )
                .await;
                assert_eq!(res.status, 201, "{}", res.body);
                res.json_str("id").unwrap()
            }
        };
        let publish = |ws: Uuid, id: String| {
            let (server, root) = (&server, &root);
            async move {
                send(
                    server,
                    "POST",
                    &format!("/api/admin/workspaces/{ws}/entries/{id}/publish"),
                    root,
                    Some("{\"revision\":1}"),
                )
                .await
            }
        };
        let a = create(ws, "a").await;
        let b = create(ws, "b").await;
        for _ in 0..2 {
            let res = publish(ws, a.clone()).await;
            assert!(res.status < 300, "{}", res.body);
        }
        // 記事ごとの上限
        let res = publish(ws, a.clone()).await;
        assert_eq!(
            code(&res),
            (409, Some("too_many_versions".to_owned())),
            "{}",
            res.body
        );
        // ワークスペースごとの上限（a に2版、b に1版で3版）
        let res = publish(ws, b.clone()).await;
        assert!(res.status < 300, "{}", res.body);
        let res = publish(ws, b.clone()).await;
        assert_eq!(
            code(&res),
            (409, Some("too_many_versions".to_owned())),
            "{}",
            res.body
        );
        // 他のワークスペースは別に数える
        let c = create(other, "c").await;
        let res = publish(other, c).await;
        assert!(res.status < 300, "{}", res.body);
    })
    .await;
}
