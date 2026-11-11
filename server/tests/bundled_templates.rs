//! 同梱のテンプレートを管理画面からデプロイする結合テスト（13-security.md「デプロイ」の同梱のテンプレート）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::content::*;
use common::server::*;
use common::with_scratch;
use serde_json::Value;

const LIST: &str = "/api/admin/bundled-templates";
const DEPLOY: &str = "/api/admin/bundled-templates/blog/deploy";
const SITE: &str = r#"{"siteUrl":"https://blog.example.org"}"#;
const SITE_DRY: &str = r#"{"siteUrl":"https://blog.example.org","dryRun":true}"#;

#[tokio::test]
async fn server_admins_deploy_the_bundled_blog_template_from_the_admin() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        create_user(&server.pool, "root@example.test", true).await;
        let root = login(&server, "root@example.test", PASSWORD).await;
        let (secret, _) = enroll(&server, &root).await;
        create_user(&server.pool, "editor@example.test", false).await;
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        add_member(&server, ws, "editor@example.test", "admin").await;

        // 一覧: まだデプロイしていない
        let res = get(&server, LIST, &root).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let list: Value = serde_json::from_str(&res.body).unwrap();
        let blog = list
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "blog")
            .unwrap();
        assert!(blog["deployedVersion"].is_null(), "{blog}");
        assert_eq!(blog["upToDate"], false);
        assert_eq!(blog["workspaces"], 1);

        // サーバー管理者だけ。個人トークンでは使えない
        assert_eq!(get(&server, LIST, &editor).await.status, 403);
        let res = send(&server, "POST", DEPLOY, &editor, Some(SITE)).await;
        assert_eq!(res.status, 403, "{}", res.body);
        let token = send(
            &server,
            "POST",
            "/api/admin/me/tokens",
            &root,
            Some(r#"{"name":"deploy","scopes":["deploy"],"workspaceIds":[]}"#),
        )
        .await
        .json_str("token")
        .unwrap();
        let bearer = format!("Bearer {token}");
        let res = request(
            &server,
            "POST",
            DEPLOY,
            &[("Authorization", &bearer)],
            Some("{}"),
        )
        .await;
        assert!(matches!(res.status, 401 | 403), "{}", res.body);

        // マニフェストは送れない（埋め込んだ中身だけをデプロイする）
        let res = send(
            &server,
            "POST",
            DEPLOY,
            &root,
            Some(r#"{"siteUrl":"https://blog.example.org","manifest":{}}"#),
        )
        .await;
        assert!(matches!(res.status, 400 | 422), "{}", res.body);
        // サイトの URL はオリジンだけ（https。http は localhost だけ）
        for bad in [
            r#"{"siteUrl":"http://blog.example.org"}"#,
            r#"{"siteUrl":"https://blog.example.org/blog"}"#,
            r#"{"siteUrl":"javascript:alert(1)"}"#,
            "{}",
        ] {
            let res = send(&server, "POST", DEPLOY, &root, Some(bad)).await;
            assert!(matches!(res.status, 400 | 422), "{bad}: {}", res.body);
        }
        // 同梱にない名前
        let res = send(
            &server,
            "POST",
            "/api/admin/bundled-templates/nope/deploy",
            &root,
            Some(SITE),
        )
        .await;
        assert_eq!(res.status, 404, "{}", res.body);

        // 試行は再認証なしでできる。本当のデプロイは再認証が要る
        expire_reauth(&server.pool, "root@example.test").await;
        let res = send(&server, "POST", DEPLOY, &root, Some(SITE_DRY)).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert!(res.body.contains("\"dryRun\":true"), "{}", res.body);
        let res = send(&server, "POST", DEPLOY, &root, Some(SITE)).await;
        assert_eq!(
            (res.status, res.json_str("error").as_deref()),
            (403, Some("reauth_required")),
            "{}",
            res.body
        );
        forget_used_steps(&server.pool, "root@example.test").await;
        let res = send(
            &server,
            "POST",
            "/api/admin/session/reauth",
            &root,
            Some(&format!("{{\"totpCode\":\"{}\"}}", code(&secret, 0))),
        )
        .await;
        assert!(res.status < 300, "{}", res.body);
        let res = send(&server, "POST", DEPLOY, &root, Some(SITE)).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert!(res.body.contains("\"state\":\"ok\""), "{}", res.body);
        // 公開URLのひな形は、入れたサイトのオリジンで始まる
        let stored: String = sqlx::query_scalar(
            "SELECT manifest->'contentTypes'->0->>'urlTemplate' FROM yomi.template_versions WHERE template = 'blog'",
        )
        .fetch_one(&server.pool)
        .await
        .unwrap();
        assert!(stored.starts_with("https://blog.example.org/"), "{stored}");

        // デプロイの後: 一覧は最新で、ワークスペースで記事を作れる
        let list: Value = serde_json::from_str(&get(&server, LIST, &root).await.body).unwrap();
        let blog = list
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "blog")
            .unwrap();
        assert_eq!(blog["deployedVersion"], 1);
        assert_eq!(blog["upToDate"], true);
        assert_eq!(blog["siteUrl"], "https://blog.example.org");
        let res = send(
            &server,
            "POST",
            &format!("/api/admin/workspaces/{ws}/entries"),
            &root,
            Some(r#"{"contentType":"post","fields":{"title":"t","slug":"s"}}"#),
        )
        .await;
        assert_eq!(res.status, 201, "{}", res.body);
        let audit: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM yomi.audit_log WHERE action = 'template.deployed'",
        )
        .fetch_one(&server.pool)
        .await
        .unwrap();
        assert_eq!(audit, 1);
    })
    .await;
}
