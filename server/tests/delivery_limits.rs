//! 配信APIの回数制限・応答の大きさの上限・共有キャッシュの結合テスト（本物のPostgreSQLとHTTPを使う）。
//!
//! 環境変数 `CMS_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

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
        assert_eq!(run(&server, ws, "cms_key_invalid", &id).await.status, 401);
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
