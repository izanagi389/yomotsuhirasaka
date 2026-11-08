//! CLI用の個人トークンの結合テスト（本物のPostgreSQLとHTTPを使う）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::server::*;
use common::with_scratch;
use sqlx::PgPool;
use uuid::Uuid;

async fn user(server: &Server, email: &str, server_admin: bool, mfa: bool) -> (Uuid, Login) {
    create_user(&server.pool, email, server_admin).await;
    let s = login(server, email, PASSWORD).await;
    if mfa {
        enroll(server, &s).await;
    }
    let id = sqlx::query_scalar("SELECT id FROM yomi.users WHERE email = $1")
        .bind(email)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    (id, s)
}

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

async fn create_token(server: &Server, s: &Login, body: &str) -> Resp {
    send(server, "POST", "/api/admin/me/tokens", s, Some(body)).await
}

async fn bearer(
    server: &Server,
    method: &str,
    path: &str,
    token: &str,
    body: Option<&str>,
) -> Resp {
    request(
        server,
        method,
        path,
        &[("Authorization", &format!("Bearer {token}"))],
        body,
    )
    .await
}

fn ids(list: &[Uuid]) -> String {
    list.iter()
        .map(|u| format!("\"{u}\""))
        .collect::<Vec<_>>()
        .join(",")
}

#[tokio::test]
async fn issuing_listing_expiring_and_revoking() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (_, root) = user(&server, "root@example.test", true, true).await;
        let a = create_ws(&server, &root, "A").await;
        let (editor_id, editor) = user(&server, "editor@example.test", false, false).await;
        add_member(&server.pool, a, editor_id, "editor").await;
        let body = format!(
            "{{\"name\":\"ci\",\"scopes\":[\"read\"],\"workspaceIds\":[{}]}}",
            ids(&[a])
        );

        // 発行: 秘密の値はこの応答だけ。一覧には出ない（INV-KEY-01）
        let res = create_token(&server, &editor, &body).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let token = res.json_str("token").unwrap();
        assert!(token.starts_with("yomi_pat_") && token.len() > 40);
        let list = get(&server, "/api/admin/me/tokens", &editor).await;
        assert_eq!(list.status, 200);
        assert!(list.body.contains("\"name\":\"ci\"") && !list.body.contains(&token));
        let stored: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM yomi.personal_tokens WHERE token_hash = convert_to($1, 'UTF8')",
        )
        .bind(&token)
        .fetch_one(&server.pool)
        .await
        .unwrap();
        assert_eq!(stored, 0, "only the hash is stored");
        let audit: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM yomi.audit_log WHERE action = 'personal_token.issued'",
        )
        .fetch_one(&server.pool)
        .await
        .unwrap();
        assert_eq!(audit, 1);

        // 使える
        let me = bearer(&server, "GET", "/api/admin/me", &token, None).await;
        assert_eq!(me.status, 200);
        assert_eq!(me.json_str("email").as_deref(), Some("editor@example.test"));

        // 有効期限の指定の誤り
        for bad in ["not-a-date", "2000-01-01T00:00:00Z", "2999-01-01T00:00:00Z"] {
            let res = create_token(
                &server,
                &editor,
                &format!("{{\"scopes\":[\"read\"],\"expiresAt\":\"{bad}\"}}"),
            )
            .await;
            assert_eq!(res.status, 400, "{bad}: {}", res.body);
        }
        // 範囲の誤り、所属していないワークスペース
        assert_eq!(
            create_token(&server, &editor, "{\"scopes\":[]}")
                .await
                .status,
            400
        );
        assert_eq!(
            create_token(&server, &editor, "{\"scopes\":[\"admin\"]}")
                .await
                .status,
            400
        );
        let res = create_token(
            &server,
            &editor,
            &format!(
                "{{\"scopes\":[\"read\"],\"workspaceIds\":[\"{}\"]}}",
                Uuid::new_v4()
            ),
        )
        .await;
        assert_eq!(res.status, 404);
        // デプロイの範囲はサーバー管理者だけ
        let res = create_token(&server, &editor, "{\"scopes\":[\"deploy\"]}").await;
        assert_eq!(
            (res.status, res.json_str("error").as_deref()),
            (403, Some("deploy_scope_forbidden"))
        );
        assert_eq!(
            create_token(&server, &root, "{\"scopes\":[\"deploy\"]}")
                .await
                .status,
            201
        );
        // 再認証の有効時間を過ぎていれば発行できない
        expire_reauth(&server.pool, "editor@example.test").await;
        assert_eq!(create_token(&server, &editor, &body).await.status, 403);

        // 期限切れ（INV-AUTH-09）
        sqlx::query("UPDATE yomi.personal_tokens SET expires_at = now() - interval '1 second'")
            .execute(&server.pool)
            .await
            .unwrap();
        assert_eq!(
            bearer(&server, "GET", "/api/admin/me", &token, None)
                .await
                .status,
            401
        );

        // 失効
        let fresh = login(&server, "editor@example.test", PASSWORD).await;
        let res = create_token(&server, &fresh, &body).await;
        let token = res.json_str("token").unwrap();
        let id = res.json_str("id").unwrap();
        assert_eq!(
            bearer(&server, "GET", "/api/admin/me", &token, None)
                .await
                .status,
            200
        );
        assert_eq!(
            send(
                &server,
                "DELETE",
                &format!("/api/admin/me/tokens/{id}"),
                &fresh,
                None
            )
            .await
            .status,
            204
        );
        assert_eq!(
            bearer(&server, "GET", "/api/admin/me", &token, None)
                .await
                .status,
            401
        );
        // 他人のトークンは失効させられない
        let (_, other) = user(&server, "other@example.test", false, false).await;
        assert_eq!(
            send(
                &server,
                "DELETE",
                &format!("/api/admin/me/tokens/{id}"),
                &other,
                None
            )
            .await
            .status,
            404
        );
    })
    .await;
}

#[tokio::test]
async fn tokens_are_limited_by_scope_workspace_and_role() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (_, root) = user(&server, "root@example.test", true, true).await;
        let a = create_ws(&server, &root, "A").await;
        let b = create_ws(&server, &root, "B").await;
        let (admin_id, admin) = user(&server, "admin@example.test", false, true).await;
        let (both_id, both) = user(&server, "both@example.test", false, false).await;
        add_member(&server.pool, a, admin_id, "admin").await;
        add_member(&server.pool, a, both_id, "editor").await;
        add_member(&server.pool, b, both_id, "editor").await;

        let read = create_token(
            &server,
            &both,
            &format!("{{\"scopes\":[\"read\"],\"workspaceIds\":[{}]}}", ids(&[a])),
        )
        .await;
        let read = read.json_str("token").unwrap();
        let write = create_token(
            &server,
            &both,
            &format!(
                "{{\"scopes\":[\"read\",\"write\"],\"workspaceIds\":[{}]}}",
                ids(&[a, b])
            ),
        )
        .await;
        let write = write.json_str("token").unwrap();

        // 読み取りだけのトークンでは書き込めない。書き込みの範囲ならCSRFなしで書ける
        let res = bearer(
            &server,
            "PATCH",
            "/api/admin/me",
            &read,
            Some("{\"locale\":\"ja\"}"),
        )
        .await;
        assert_eq!(
            (res.status, res.json_str("error").as_deref()),
            (403, Some("insufficient_scope"))
        );
        assert_eq!(
            bearer(
                &server,
                "PATCH",
                "/api/admin/me",
                &write,
                Some("{\"locale\":\"ja\"}")
            )
            .await
            .status,
            200
        );

        // 選んだワークスペースだけ（INV-AUTH-09）
        let list = bearer(&server, "GET", "/api/admin/workspaces", &read, None).await;
        assert!(
            list.body.contains(&a.to_string()) && !list.body.contains(&b.to_string()),
            "{}",
            list.body
        );
        assert_eq!(
            bearer(
                &server,
                "GET",
                &format!("/api/admin/workspaces/{b}"),
                &read,
                None
            )
            .await
            .status,
            404
        );
        assert_eq!(
            bearer(
                &server,
                "GET",
                &format!("/api/admin/workspaces/{b}"),
                &write,
                None
            )
            .await
            .status,
            200
        );

        // 本人のロールを超えられない。外されたら直ちに使えない（INV-AUTH-18）
        assert_eq!(
            bearer(
                &server,
                "GET",
                &format!("/api/admin/workspaces/{a}/members"),
                &write,
                None
            )
            .await
            .status,
            403
        );
        sqlx::query("DELETE FROM yomi.memberships WHERE workspace_id = $1 AND user_id = $2")
            .bind(a)
            .bind(both_id)
            .execute(&server.pool)
            .await
            .unwrap();
        assert_eq!(
            bearer(
                &server,
                "GET",
                &format!("/api/admin/workspaces/{a}"),
                &write,
                None
            )
            .await
            .status,
            404
        );
        assert_eq!(
            bearer(
                &server,
                "GET",
                &format!("/api/admin/workspaces/{b}"),
                &write,
                None
            )
            .await
            .status,
            200
        );

        // 管理者のトークンでも、再認証の要る重い操作はできない
        let admin_token = create_token(
            &server,
            &admin,
            &format!(
                "{{\"scopes\":[\"read\",\"write\"],\"workspaceIds\":[{}]}}",
                ids(&[a])
            ),
        )
        .await;
        let admin_token = admin_token.json_str("token").unwrap();
        assert_eq!(
            bearer(
                &server,
                "GET",
                &format!("/api/admin/workspaces/{a}/members"),
                &admin_token,
                None
            )
            .await
            .status,
            200
        );
        let res = bearer(
            &server,
            "PATCH",
            &format!("/api/admin/workspaces/{a}/members/{admin_id}"),
            &admin_token,
            Some("{\"canViewMembers\":true}"),
        )
        .await;
        assert_eq!(res.status, 403);

        // サーバー管理者のトークンでも、インスタンス全体の操作や、所属していないワークスペースは扱えない
        let root_token = create_token(&server, &root, "{\"scopes\":[\"read\",\"write\"]}").await;
        let root_token = root_token.json_str("token").unwrap();
        assert_eq!(
            bearer(&server, "GET", "/api/admin/users", &root_token, None)
                .await
                .status,
            403
        );
        assert_eq!(
            bearer(
                &server,
                "POST",
                "/api/admin/workspaces",
                &root_token,
                Some("{\"name\":\"X\"}")
            )
            .await
            .status,
            403
        );
        assert_eq!(
            bearer(
                &server,
                "GET",
                &format!("/api/admin/workspaces/{a}"),
                &root_token,
                None
            )
            .await
            .status,
            404
        );
        assert_eq!(
            bearer(&server, "GET", "/api/admin/workspaces", &root_token, None)
                .await
                .body,
            "[]"
        );

        // 管理画面だけの操作は、トークンではできない
        for (method, path, body) in [
            ("DELETE", "/api/admin/session", None),
            ("GET", "/api/admin/me/sessions", None),
            ("GET", "/api/admin/me/tokens", None),
            (
                "POST",
                "/api/admin/me/tokens",
                Some("{\"scopes\":[\"read\"]}"),
            ),
            (
                "POST",
                "/api/admin/session/reauth",
                Some("{\"password\":\"x\"}"),
            ),
            ("POST", "/api/admin/me/mfa/totp", None),
        ] {
            let res = bearer(&server, method, path, &write, body).await;
            assert_eq!(
                (res.status, res.json_str("error").as_deref()),
                (403, Some("session_required")),
                "{method} {path}"
            );
        }

        // Authorization ヘッダーがあればCookieは読まない
        let res = request(
            &server,
            "GET",
            "/api/admin/me",
            &[
                ("Authorization", "Bearer yomi_pat_wrong"),
                ("Cookie", &both.cookie_header()),
            ],
            None,
        )
        .await;
        assert_eq!(res.status, 401);
    })
    .await;
}

#[tokio::test]
async fn credentials_in_urls_are_rejected() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (_, s) = user(&server, "url@example.test", false, false).await;
        let token = create_token(&server, &s, "{\"scopes\":[\"read\"]}")
            .await
            .json_str("token")
            .unwrap();
        for name in ["token", "access_token", "api_key", "key"] {
            // クエリだけでも、ヘッダーと両方でも拒否する（INV-AUTH-11）
            let only = request(
                &server,
                "GET",
                &format!("/api/admin/me?{name}={token}"),
                &[],
                None,
            )
            .await;
            assert_eq!(
                (only.status, only.json_str("error").as_deref()),
                (401, Some("credential_in_url"))
            );
            let both = bearer(
                &server,
                "GET",
                &format!("/api/admin/me?{name}={token}"),
                &token,
                None,
            )
            .await;
            assert_eq!(both.status, 401);
            let key = request(
                &server,
                "GET",
                &format!("/api/admin/me?{name}=yomi_key_abc"),
                &[],
                None,
            )
            .await;
            assert_eq!(key.status, 401);
        }
        // 対照: 普通のクエリは通る
        assert_eq!(
            bearer(&server, "GET", "/api/admin/me?page=2", &token, None)
                .await
                .status,
            200
        );
    })
    .await;
}
