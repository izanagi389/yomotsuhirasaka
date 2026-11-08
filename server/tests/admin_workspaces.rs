//! ワークスペースと所属の結合テスト（本物のPostgreSQLとHTTPを使う）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::server::*;
use common::with_scratch;
use sqlx::PgPool;
use uuid::Uuid;

/// 利用者を作ってログインし、必要ならTOTPを設定する。設定の直後なので再認証の有効時間内。
async fn user(server: &Server, email: &str, server_admin: bool, mfa: bool) -> (Uuid, Login) {
    create_user(&server.pool, email, server_admin).await;
    let s = login(server, email, PASSWORD).await;
    if mfa {
        enroll(server, &s).await;
    }
    let id: Uuid = sqlx::query_scalar("SELECT id FROM yomi.users WHERE email = $1")
        .bind(email)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    (id, s)
}

/// 招待の承諾ができるまで、所属はDBで直接作る（テストの準備のため）。
async fn add_member(pool: &PgPool, ws: Uuid, user: Uuid, role: &str) {
    sqlx::query("INSERT INTO yomi.memberships (workspace_id, user_id, role) VALUES ($1, $2, $3)")
        .bind(ws)
        .bind(user)
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
}

async fn create_ws(server: &Server, s: &Login, name: &str) -> Uuid {
    let res = send(
        server,
        "POST",
        "/api/admin/workspaces",
        s,
        Some(&format!("{{\"name\":\"{name}\"}}")),
    )
    .await;
    assert_eq!(res.status, 201, "{}", res.body);
    res.json_str("id").unwrap().parse().unwrap()
}

async fn audit_count(
    pool: &PgPool,
    ws: Option<Uuid>,
    actor: Uuid,
    action: &str,
    target: Option<Uuid>,
) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM yomi.audit_log
         WHERE workspace_id IS NOT DISTINCT FROM $1 AND actor_user_id = $2 AND action = $3
           AND ($4::text IS NULL OR $4::text = ANY (target_ids))",
    )
    .bind(ws)
    .bind(actor)
    .bind(action)
    .bind(target.map(|t| t.to_string()))
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn only_server_admins_with_mfa_create_workspaces() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (_, plain_admin) = user(&server, "root-nomfa@example.test", true, false).await;
        let (_, member) = user(&server, "member@example.test", false, true).await;
        let (root_id, root) = user(&server, "root@example.test", true, true).await;
        let body = Some("{\"name\":\"Blog\"}");

        let res = send(&server, "POST", "/api/admin/workspaces", &plain_admin, body).await;
        assert_eq!(
            (res.status, res.json_str("error").as_deref()),
            (403, Some("mfa_required"))
        );
        let res = send(&server, "POST", "/api/admin/workspaces", &member, body).await;
        assert_eq!(
            (res.status, res.json_str("error").as_deref()),
            (403, Some("forbidden"))
        );
        let res = send(
            &server,
            "POST",
            "/api/admin/workspaces",
            &root,
            Some("{\"name\":\"  \"}"),
        )
        .await;
        assert_eq!(res.status, 400);
        let res = send(
            &server,
            "POST",
            "/api/admin/workspaces",
            &root,
            Some("{\"name\":\"x\",\"template\":\"Bad Name\"}"),
        )
        .await;
        assert_eq!(res.status, 400);

        let ws = create_ws(&server, &root, "Blog").await;
        // スキーマとロールができ、監査ログに残る
        let schema: String =
            sqlx::query_scalar("SELECT db_schema FROM yomi.workspaces WHERE id = $1")
                .bind(ws)
                .fetch_one(&server.pool)
                .await
                .unwrap();
        assert_eq!(schema, format!("ws_{}", ws.simple()));
        assert_eq!(
            audit_count(
                &server.pool,
                Some(ws),
                root_id,
                "workspace.created",
                Some(ws)
            )
            .await,
            1
        );

        // 一覧: サーバー管理者には見え（ロールは null）、所属していない人には見えない
        let list = get(&server, "/api/admin/workspaces", &root).await;
        assert!(
            list.body.contains(&ws.to_string()) && list.body.contains("\"role\":null"),
            "{}",
            list.body
        );
        let list = get(&server, "/api/admin/workspaces", &member).await;
        assert_eq!(list.body, "[]");

        // 再認証の有効時間を過ぎると作れない
        expire_reauth(&server.pool, "root@example.test").await;
        let res = send(&server, "POST", "/api/admin/workspaces", &root, body).await;
        assert_eq!(
            (res.status, res.json_str("error").as_deref()),
            (403, Some("reauth_required"))
        );
    })
    .await;
}

#[tokio::test]
async fn members_roles_and_isolation() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root_id, root) = user(&server, "root@example.test", true, true).await;
        let a = create_ws(&server, &root, "A").await;
        let b = create_ws(&server, &root, "B").await;
        let (admin_a_id, admin_a) = user(&server, "admin-a@example.test", false, true).await;
        let (editor_id, editor) = user(&server, "editor-a@example.test", false, false).await;
        let (nomfa_id, nomfa_admin) =
            user(&server, "admin-a-nomfa@example.test", false, false).await;
        let (admin_b_id, _) = user(&server, "admin-b@example.test", false, true).await;
        add_member(&server.pool, a, admin_a_id, "admin").await;
        add_member(&server.pool, a, editor_id, "editor").await;
        add_member(&server.pool, a, nomfa_id, "viewer").await;
        add_member(&server.pool, b, admin_b_id, "admin").await;

        // 自分の所属するワークスペースだけが見える
        let list = get(&server, "/api/admin/workspaces", &admin_a).await;
        assert!(
            list.body.contains(&a.to_string()) && !list.body.contains(&b.to_string()),
            "{}",
            list.body
        );
        assert!(list.body.contains("\"role\":\"admin\""));
        // 所属していないワークスペースは、存在しないものと区別できない
        let other = get(&server, &format!("/api/admin/workspaces/{b}"), &admin_a).await;
        let missing = get(
            &server,
            &format!("/api/admin/workspaces/{}", Uuid::new_v4()),
            &admin_a,
        )
        .await;
        assert_eq!((other.status, &other.body), (404, &missing.body));
        assert_eq!(
            get(&server, &format!("/api/admin/workspaces/{a}"), &editor)
                .await
                .status,
            200
        );

        // 所属者の一覧: 管理者だけ。他のワークスペースの利用者は存在を知らせない（INV-TEN-04）
        let members = get(
            &server,
            &format!("/api/admin/workspaces/{a}/members"),
            &admin_a,
        )
        .await;
        assert_eq!(members.status, 200);
        assert_eq!(members.body.matches("\"userId\"").count(), 3);
        assert!(!members.body.contains("admin-b@"));
        let outsider = get(
            &server,
            &format!("/api/admin/workspaces/{a}/members/{admin_b_id}"),
            &admin_a,
        )
        .await;
        let nobody = get(
            &server,
            &format!("/api/admin/workspaces/{a}/members/{}", Uuid::new_v4()),
            &admin_a,
        )
        .await;
        assert_eq!((outsider.status, &outsider.body), (404, &nobody.body));
        assert_eq!(
            get(
                &server,
                &format!("/api/admin/workspaces/{a}/members"),
                &editor
            )
            .await
            .status,
            403
        );
        assert_eq!(
            get(
                &server,
                &format!("/api/admin/workspaces/{b}/members"),
                &admin_a
            )
            .await
            .status,
            404
        );

        // 多要素認証のない管理者は、管理操作ができない（INV-AUTH-05）
        sqlx::query("UPDATE yomi.memberships SET role = 'admin' WHERE user_id = $1")
            .bind(nomfa_id)
            .execute(&server.pool)
            .await
            .unwrap();
        let res = get(
            &server,
            &format!("/api/admin/workspaces/{a}/members"),
            &nomfa_admin,
        )
        .await;
        assert_eq!(
            (res.status, res.json_str("error").as_deref()),
            (403, Some("mfa_required"))
        );
        sqlx::query("UPDATE yomi.memberships SET role = 'viewer' WHERE user_id = $1")
            .bind(nomfa_id)
            .execute(&server.pool)
            .await
            .unwrap();

        // ロールの変更は記録される。誤ったロールは400
        let path = format!("/api/admin/workspaces/{a}/members/{editor_id}");
        let res = send(
            &server,
            "PATCH",
            &path,
            &admin_a,
            Some("{\"role\":\"author\"}"),
        )
        .await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert_eq!(res.json_str("role").as_deref(), Some("author"));
        assert_eq!(
            audit_count(
                &server.pool,
                Some(a),
                admin_a_id,
                "member.role_changed",
                Some(editor_id)
            )
            .await,
            1
        );
        assert_eq!(
            send(
                &server,
                "PATCH",
                &path,
                &admin_a,
                Some("{\"role\":\"owner\"}")
            )
            .await
            .status,
            400
        );
        // 他のワークスペースの所属者は変えられない
        let res = send(
            &server,
            "PATCH",
            &format!("/api/admin/workspaces/{b}/members/{admin_b_id}"),
            &admin_a,
            Some("{\"role\":\"viewer\"}"),
        )
        .await;
        assert_eq!(res.status, 404);
        // 最後の管理者は外せない
        let me = format!("/api/admin/workspaces/{a}/members/{admin_a_id}");
        assert_eq!(
            send(
                &server,
                "PATCH",
                &me,
                &admin_a,
                Some("{\"role\":\"editor\"}")
            )
            .await
            .status,
            409
        );
        assert_eq!(
            send(&server, "DELETE", &me, &admin_a, None).await.status,
            409
        );

        // 外すと、同じセッションでも直ちに見えなくなる
        assert_eq!(
            send(&server, "DELETE", &path, &admin_a, None).await.status,
            204
        );
        assert_eq!(
            audit_count(
                &server.pool,
                Some(a),
                admin_a_id,
                "member.removed",
                Some(editor_id)
            )
            .await,
            1
        );
        assert_eq!(
            get(&server, &format!("/api/admin/workspaces/{a}"), &editor)
                .await
                .status,
            404
        );
        assert_eq!(
            send(&server, "DELETE", &path, &admin_a, None).await.status,
            404
        );

        // サーバー管理者は所属していなくても操作でき、そのワークスペースの監査ログに残る（Q122）
        let res = send(
            &server,
            "PATCH",
            &format!("/api/admin/workspaces/{a}/members/{nomfa_id}"),
            &root,
            Some("{\"role\":\"editor\"}"),
        )
        .await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert_eq!(
            audit_count(
                &server.pool,
                Some(a),
                root_id,
                "member.role_changed",
                Some(nomfa_id)
            )
            .await,
            1
        );

        // インスタンスの利用者の一覧はサーバー管理者だけ
        assert_eq!(get(&server, "/api/admin/users", &admin_a).await.status, 403);
        let users = get(&server, "/api/admin/users", &root).await;
        assert_eq!(users.status, 200);
        assert!(users.body.contains("admin-b@example.test"));
        assert!(!users.body.contains("argon2"));

        // ログインは監査ログに残る（インスタンス全体の記録）
        assert!(audit_count(&server.pool, None, admin_a_id, "auth.login", None).await >= 1);
    })
    .await;
}
