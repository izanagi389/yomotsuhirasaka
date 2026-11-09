//! 配信APIの回数制限・応答の大きさの上限・共有キャッシュの結合テスト（本物のPostgreSQLとHTTPを使う）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::content::*;
use common::server::*;
use common::with_scratch;
use serde_json::json;
use uuid::Uuid;

async fn api_key(
    server: &Server,
    root: &Login,
    ws: Uuid,
    scopes: &[&str],
    purpose: &str,
) -> String {
    let body = json!({"scopes": scopes, "purpose": purpose}).to_string();
    let res = send(
        server,
        "POST",
        &format!("/api/admin/workspaces/{ws}/api-keys"),
        root,
        Some(&body),
    )
    .await;
    assert_eq!(res.status, 201, "{}", res.body);
    res.json_str("key").unwrap()
}

async fn register(server: &Server, ws: Uuid, key: &str, query: &str) -> String {
    let res = request(
        server,
        "POST",
        &format!("/ws/{ws}/graphql/queries"),
        &[("Authorization", &format!("Bearer {key}"))],
        Some(&json!({"query": query}).to_string()),
    )
    .await;
    assert_eq!(res.status, 201, "{}", res.body);
    res.json_str("id").unwrap()
}

async fn run(server: &Server, ws: Uuid, key: &str, id: &str) -> Resp {
    request(
        server,
        "GET",
        &format!("/ws/{ws}/graphql?id={id}"),
        &[("Authorization", &format!("Bearer {key}"))],
        None,
    )
    .await
}

const LIST: &str = "{ postList(locale: \"ja\") { id title } }";

#[tokio::test]
async fn rate_limits_per_key_and_workspace() {
    with_scratch(|app, migration| async move {
        let mut cfg = config(&app, &migration);
        cfg.delivery.build_key_requests_per_minute = 3;
        cfg.delivery.server_key_requests_per_minute = 4;
        cfg.delivery.workspace_requests_per_minute = 6;
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
        let build = api_key(&server, &root, ws, &["read", "register"], "build").await;
        let ssr = api_key(&server, &root, ws, &["read"], "server").await;
        let other_key = api_key(&server, &root, other, &["read", "register"], "build").await;

        // 登録も1回に数える。ビルド用のキーは3回まで（INV-GQL-03）
        let id = register(&server, ws, &build, LIST).await;
        for _ in 0..2 {
            assert_eq!(run(&server, ws, &build, &id).await.status, 200);
        }
        let res = run(&server, ws, &build, &id).await;
        assert_eq!(
            (res.status, res.json_str("error").as_deref()),
            (429, Some("rate_limited"))
        );
        assert!(res.header("retry-after").is_some());
        // サーバー配信用のキーは別に数え、上限も別。ワークスペースの合計（6回）で止まる
        for _ in 0..3 {
            assert_eq!(run(&server, ws, &ssr, &id).await.status, 200);
        }
        assert_eq!(
            run(&server, ws, &ssr, &id).await.status,
            429,
            "the workspace total is 6"
        );
        // 他のワークスペースは止まらない（INV-TEN-03）
        let other_id = register(&server, other, &other_key, LIST).await;
        assert_eq!(run(&server, other, &other_key, &other_id).await.status, 200);
        // 照合できないキーは数えない
        assert_eq!(run(&server, ws, "yomi_key_invalid", &id).await.status, 401);
    })
    .await;
}

#[tokio::test]
async fn response_size_and_shared_cache() {
    with_scratch(|app, migration| async move {
        let mut cfg = config(&app, &migration);
        cfg.delivery.max_response_bytes = 300;
        cfg.delivery.shared_cache = true;
        cfg.delivery.shared_cache_seconds = 30;
        let server = start(cfg).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(deploy(&server, &token, "blog", &blog_manifest(), "").await.status, 200);
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        create_user(&server.pool, "editor@example.test", false).await;
        add_member(&server, ws, "editor@example.test", "editor").await;
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        let read_only = api_key(&server, &root, ws, &["read"], "build").await;
        let both = api_key(&server, &root, ws, &["read", "register"], "build").await;
        let id = register(&server, ws, &both, LIST).await;

        // 共有キャッシュ: 公開版の読み取りだけのキーの応答だけに許す（INV-GQL-08）
        let res = run(&server, ws, &read_only, &id).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert_eq!(res.header("cache-control"), Some("public, max-age=0, s-maxage=30"));
        assert_eq!(run(&server, ws, &both, &id).await.header("cache-control"), Some("private, no-cache"));

        // 応答の大きさの上限: 超える応答は返さない
        for i in 0..5 {
            let body = json!({"contentType": "post", "fields": {"title": "x".repeat(100), "slug": format!("s{i}")}}).to_string();
            let res = send(&server, "POST", &format!("/api/admin/workspaces/{ws}/entries"), &editor, Some(&body)).await;
            let entry = res.json_str("id").unwrap();
            let res = send(&server, "POST", &format!("/api/admin/workspaces/{ws}/entries/{entry}/publish"), &editor, Some("{\"revision\":1}")).await;
            assert_eq!(res.status, 200, "{}", res.body);
        }
        let res = run(&server, ws, &read_only, &id).await;
        assert_eq!((res.status, res.json_str("error").as_deref()), (422, Some("response_too_large")));
        assert!(!res.body.contains("xxxxxxxxxx"));
    })
    .await;
}

const KEYWORD_SEARCH: &str =
    "{ search(text: \"x\", locale: \"ja\", first: 5, mode: KEYWORD) { keywordSearch } }";

/// 接続のプール（16本）より多い同時の検索でも、詰まらない（検索語の準備の間は接続を持たない）。
#[tokio::test]
async fn concurrent_searches_do_not_exhaust_the_pool() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(
            deploy(&server, &token, "blog", &blog_manifest(), "")
                .await
                .status,
            200
        );
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        let key = api_key(&server, &root, ws, &["read", "register"], "server").await;
        let id = register(&server, ws, &key, KEYWORD_SEARCH).await;
        let started = std::time::Instant::now();
        let all = (0..32).map(|_| run(&server, ws, &key, &id));
        for res in async_graphql::futures_util::future::join_all(all).await {
            assert_eq!(res.status, 200, "{}", res.body);
            assert!(!res.body.contains("errors"), "{}", res.body);
        }
        // 詰まると、接続の取得待ち（10秒）まで止まる
        assert!(started.elapsed().as_secs() < 8, "{:?}", started.elapsed());
    })
    .await;
}

#[tokio::test]
async fn queries_have_time_and_size_limits() {
    with_scratch(|app, migration| async move {
        let mut cfg = config(&app, &migration);
        cfg.delivery.max_resolved_entries = 0;
        let server = start(cfg).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(deploy(&server, &token, "blog", &blog_manifest(), "").await.status, 200);
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        let body = json!({"contentType": "post", "fields": {"title": "t", "slug": "s", "body": {"type": "doc", "content": []}}});
        let res = send(&server, "POST", &format!("/api/admin/workspaces/{ws}/entries"), &root, Some(&body.to_string())).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let entry = res.json_str("id").unwrap();
        let res = send(&server, "POST", &format!("/api/admin/workspaces/{ws}/entries/{entry}/publish"), &root, Some("{\"revision\":1}")).await;
        assert!(res.status < 300, "{}", res.body);
        let key = api_key(&server, &root, ws, &["read", "register"], "server").await;
        let id = register(&server, ws, &key, LIST).await;
        let res = run(&server, ws, &key, &id).await;
        assert_eq!((res.status, res.json_str("error").as_deref()), (422, Some("query_too_large")), "{}", res.body);
    })
    .await;

    with_scratch(|app, migration| async move {
        let mut cfg = config(&app, &migration);
        cfg.delivery.max_query_seconds = 0;
        let server = start(cfg).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(
            deploy(&server, &token, "blog", &blog_manifest(), "")
                .await
                .status,
            200
        );
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        let key = api_key(&server, &root, ws, &["read", "register"], "server").await;
        let id = register(&server, ws, &key, LIST).await;
        let res = run(&server, ws, &key, &id).await;
        assert_eq!(
            (res.status, res.json_str("error").as_deref()),
            (503, Some("query_timeout")),
            "{}",
            res.body
        );
    })
    .await;
}

/// 無効なキーを送り続ける接続元は断る（ほかの接続元には影響しない）。
#[tokio::test]
async fn invalid_keys_are_limited_per_address() {
    with_scratch(|app, migration| async move {
        let mut cfg = config(&app, &migration);
        cfg.limits.invalid_api_keys_per_ip_per_minute = 2;
        cfg.trusted_proxies = vec!["127.0.0.1".parse().unwrap()];
        let server = start(cfg).await;
        let ws = Uuid::new_v4();
        let from = |ip: &'static str| {
            let server = &server;
            async move {
                request(
                    server,
                    "GET",
                    &format!("/ws/{ws}/graphql?id=x"),
                    &[
                        ("Authorization", "Bearer yomi_key_invalid"),
                        ("X-Forwarded-For", ip),
                    ],
                    None,
                )
                .await
                .status
            }
        };
        assert_eq!(from("198.18.90.1").await, 401);
        assert_eq!(from("198.18.90.1").await, 401);
        assert_eq!(from("198.18.90.1").await, 429);
        assert_eq!(from("198.18.90.2").await, 401);
    })
    .await;
}
