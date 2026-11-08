//! 配信APIのキーの結合テスト（本物のPostgreSQLとHTTPを使う）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。
//! キーを使う配信APIは後の段階で作る。ここでは照合の関数（api_keys::authenticate）を直接確かめる。

mod common;

use common::server::*;
use common::with_scratch;
use sqlx::PgPool;
use uuid::Uuid;
use yomi_server::api_keys::{self, Denied};

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

#[tokio::test]
async fn managing_and_checking_api_keys() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (_, root) = user(&server, "root@example.test", true, true).await;
        let a = create_ws(&server, &root, "A").await;
        let b = create_ws(&server, &root, "B").await;
        let (admin_id, admin) = user(&server, "admin@example.test", false, true).await;
        let (editor_id, editor) = user(&server, "editor@example.test", false, false).await;
        let (nomfa_id, nomfa) = user(&server, "admin-nomfa@example.test", false, false).await;
        add_member(&server.pool, a, admin_id, "admin").await;
        add_member(&server.pool, a, editor_id, "editor").await;
        add_member(&server.pool, a, nomfa_id, "admin").await;
        let keys_a = format!("/api/admin/workspaces/{a}/api-keys");
        let body = "{\"name\":\"site\",\"scopes\":[\"read\"]}";

        // 管理者（多要素認証あり）だけが扱える
        assert_eq!(send(&server, "POST", &keys_a, &editor, Some(body)).await.status, 403);
        assert_eq!(send(&server, "POST", &keys_a, &nomfa, Some(body)).await.status, 403);
        assert_eq!(send(&server, "POST", &format!("/api/admin/workspaces/{b}/api-keys"), &admin, Some(body)).await.status, 404);
        for bad in [
            "{\"scopes\":[]}",
            "{\"scopes\":[\"write\"]}",
            "{\"scopes\":[\"read\"],\"purpose\":\"cdn\"}",
            "{\"scopes\":[\"read\"],\"expiresAt\":\"2000-01-01T00:00:00Z\"}",
            "{\"scopes\":[\"read\"],\"expiresAt\":\"tomorrow\"}",
        ] {
            assert_eq!(send(&server, "POST", &keys_a, &admin, Some(bad)).await.status, 400, "{bad}");
        }

        // 発行: 秘密の値はこの応答だけ（INV-KEY-01）
        let res = send(&server, "POST", &keys_a, &admin, Some(body)).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let read_key = res.json_str("key").unwrap();
        assert!(read_key.starts_with("yomi_key_"));
        assert_eq!(res.json_str("purpose").as_deref(), Some("build"));
        let read_id: Uuid = res.json_str("id").unwrap().parse().unwrap();
        let list = get(&server, &keys_a, &admin).await;
        assert!(list.body.contains("\"name\":\"site\"") && !list.body.contains(&read_key), "{}", list.body);
        let audit: i64 = sqlx::query_scalar("SELECT count(*) FROM yomi.audit_log WHERE workspace_id = $1 AND action = 'api_key.issued'")
            .bind(a)
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert_eq!(audit, 1);
        let res = send(&server, "POST", &keys_a, &admin, Some("{\"scopes\":[\"register\"],\"purpose\":\"server\"}")).await;
        let register_key = res.json_str("key").unwrap();

        // 照合: ワークスペースとスコープ（INV-API-01, 02、Q125）
        let check = |key: Option<String>, ws: Uuid, scope: &'static str| {
            let pool = server.pool.clone();
            async move { api_keys::authenticate(&pool, key.as_deref(), ws, scope).await.unwrap() }
        };
        let grant = check(Some(read_key.clone()), a, "read").await.unwrap();
        assert_eq!((grant.key_id, grant.workspace_id), (read_id, a));
        assert_eq!(check(None, a, "read").await.unwrap_err(), Denied::Invalid);
        assert_eq!(check(Some("yomi_key_wrong".into()), a, "read").await.unwrap_err(), Denied::Invalid);
        assert_eq!(check(Some(read_key.clone()), b, "read").await.unwrap_err(), Denied::WrongWorkspace);
        assert_eq!(check(Some(read_key.clone()), a, "register").await.unwrap_err(), Denied::MissingScope);
        assert_eq!(check(Some(register_key.clone()), a, "read").await.unwrap_err(), Denied::MissingScope);
        assert!(check(Some(register_key), a, "register").await.is_ok());

        // 有効期限（INV-KEY-04）
        let res = send(&server, "POST", &keys_a, &admin, Some("{\"scopes\":[\"read\"],\"expiresAt\":\"2999-01-01T00:00:00Z\"}")).await;
        assert_eq!(res.status, 201);
        let expiring = res.json_str("key").unwrap();
        assert!(check(Some(expiring.clone()), a, "read").await.is_ok());
        sqlx::query("UPDATE yomi.api_keys SET expires_at = now() - interval '1 second' WHERE expires_at IS NOT NULL")
            .execute(&server.pool)
            .await
            .unwrap();
        assert_eq!(check(Some(expiring), a, "read").await.unwrap_err(), Denied::Invalid);

        // 失効
        let path = format!("{keys_a}/{read_id}");
        assert_eq!(send(&server, "DELETE", &path, &editor, None).await.status, 403);
        assert_eq!(send(&server, "DELETE", &format!("/api/admin/workspaces/{b}/api-keys/{read_id}"), &root, None).await.status, 404);
        assert_eq!(send(&server, "DELETE", &path, &admin, None).await.status, 204);
        assert_eq!(check(Some(read_key), a, "read").await.unwrap_err(), Denied::Invalid);
        assert_eq!(send(&server, "DELETE", &path, &admin, None).await.status, 404);

        // 再認証の有効時間を過ぎていれば発行できない
        expire_reauth(&server.pool, "admin@example.test").await;
        assert_eq!(send(&server, "POST", &keys_a, &admin, Some(body)).await.status, 403);
    })
    .await;
}
