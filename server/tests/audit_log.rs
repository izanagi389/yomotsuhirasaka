//! 監査ログの閲覧の結合テスト（本物のPostgreSQLとHTTPを使う）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::content::*;
use common::server::*;
use common::with_scratch;
use serde_json::Value;

fn parse(res: &Resp) -> Value {
    serde_json::from_str(&res.body).unwrap_or_else(|_| panic!("{}", res.body))
}

async fn user_id(server: &Server, email: &str) -> String {
    sqlx::query_scalar::<_, uuid::Uuid>("SELECT id FROM yomi.users WHERE email = $1")
        .bind(email)
        .fetch_one(&server.pool)
        .await
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn reading_audit_logs_by_scope_and_page() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(
            deploy(&server, &token, "blog", &blog_manifest(), "")
                .await
                .status,
            200
        );
        let a = workspace_with_template(&server, &root, "A", "blog").await;
        let b = workspace_with_template(&server, &root, "B", "blog").await;
        create_user(&server.pool, "admin@example.test", false).await;
        add_member(&server, a, "admin@example.test", "admin").await;
        let admin = login(&server, "admin@example.test", PASSWORD).await;
        enroll(&server, &admin).await;
        create_user(&server.pool, "editor@example.test", false).await;
        add_member(&server, a, "editor@example.test", "editor").await;
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        let editor_id = user_id(&server, "editor@example.test").await;
        let log_a = format!("/api/admin/workspaces/{a}/audit-log");

        // 所属の変更を何件か記録する（ワークスペースの記録）
        for role in ["author", "viewer", "editor"] {
            let body = format!("{{\"role\":\"{role}\"}}");
            let res = send(
                &server,
                "PATCH",
                &format!("/api/admin/workspaces/{a}/members/{editor_id}"),
                &admin,
                Some(&body),
            )
            .await;
            assert_eq!(res.status, 200, "{}", res.body);
        }

        // 管理者は自分のワークスペースだけ。編集者・他のワークスペース・インスタンスは読めない
        let res = get(&server, &log_a, &admin).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let all = parse(&res);
        let entries = all["entries"].as_array().unwrap();
        assert!(entries.iter().all(|e| e["workspaceId"] == a.to_string()));
        assert_eq!(
            entries
                .iter()
                .filter(|e| e["action"] == "member.role_changed")
                .count(),
            3
        );
        assert_eq!(get(&server, &log_a, &editor).await.status, 403);
        assert!(matches!(
            get(
                &server,
                &format!("/api/admin/workspaces/{b}/audit-log"),
                &admin
            )
            .await
            .status,
            403 | 404
        ));
        assert_eq!(
            get(&server, "/api/admin/audit-log", &admin).await.status,
            403
        );

        // 新しい順に、limit と before で読み進める。範囲の外の before では何も返さない
        let first = parse(&get(&server, &format!("{log_a}?limit=2"), &admin).await);
        assert_eq!(first["entries"].as_array().unwrap().len(), 2);
        assert_eq!(first["entries"][0]["id"], entries[0]["id"]);
        let next = first["nextBefore"].as_str().unwrap();
        let second =
            parse(&get(&server, &format!("{log_a}?limit=500&before={next}"), &admin).await);
        assert_eq!(second["entries"][0]["id"], entries[2]["id"]);
        assert!(second["nextBefore"].is_null());
        for bad in [
            "?limit=0",
            "?limit=501",
            "?limit=x",
            "?before=x",
            "?other=1",
        ] {
            assert_eq!(
                get(&server, &format!("{log_a}{bad}"), &admin).await.status,
                400,
                "{bad}"
            );
        }
        let instance = parse(&get(&server, "/api/admin/audit-log", &root).await);
        let in_instance = instance["entries"][0]["id"].as_str().unwrap().to_owned();
        let crossed = parse(&get(&server, &format!("{log_a}?before={in_instance}"), &admin).await);
        assert_eq!(crossed["entries"].as_array().unwrap().len(), 0);

        // サーバー管理者がインスタンスの操作としてリセットを発行しても、所属するワークスペースの記録に残る（Q122）
        let res = send(
            &server,
            "POST",
            &format!("/api/admin/users/{editor_id}/password-reset"),
            &root,
            None,
        )
        .await;
        assert_eq!(res.status, 201, "{}", res.body);
        let latest = parse(&get(&server, &log_a, &admin).await);
        assert_eq!(latest["entries"][0]["action"], "password_reset.issued");
        assert_eq!(latest["entries"][0]["targetIds"][0], editor_id);
        // 監査ログを書き換える操作はない
        for method in ["PUT", "PATCH", "DELETE", "POST"] {
            assert_eq!(
                send(&server, method, &log_a, &admin, Some("{}"))
                    .await
                    .status,
                405,
                "{method}"
            );
        }
    })
    .await;
}
