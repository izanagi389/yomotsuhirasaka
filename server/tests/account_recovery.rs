//! 乗っ取りからの復旧と、利用者を止めること（R7, R8）、デプロイのトークンの制限（R10）の結合テスト。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::server::*;
use common::with_scratch;
use sqlx::PgPool;
use uuid::Uuid;
use yomi_server::auth::users;

async fn user_id(pool: &PgPool, email: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM yomi.users WHERE email = $1")
        .bind(email)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn link(pool: &PgPool, user: Uuid, sub: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO yomi.external_identities (id, user_id, issuer, subject)
         VALUES (gen_random_uuid(), $1, 'https://idp-' || $2 || '.example.test', $2) RETURNING id",
    )
    .bind(user)
    .bind(sub)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn new_token(server: &Server, s: &Login) -> String {
    let res = send(
        server,
        "POST",
        "/api/admin/me/tokens",
        s,
        Some(r#"{"scopes":["read"]}"#),
    )
    .await;
    assert_eq!(res.status, 201, "{}", res.body);
    res.json_str("token").unwrap()
}

async fn me_with_token(server: &Server, token: &str) -> u16 {
    request(
        server,
        "GET",
        "/api/admin/me",
        &[("Authorization", &format!("Bearer {token}"))],
        None,
    )
    .await
    .status
}

async fn identities(pool: &PgPool, user: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM yomi.external_identities WHERE user_id = $1")
        .bind(user)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn in_days(pool: &PgPool, days: i32) -> String {
    sqlx::query_scalar(
        r#"SELECT to_char((now() + make_interval(days => $1)) AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"')"#,
    )
    .bind(days)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn recovery_removes_what_an_attacker_left() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let email = "victim@example.test";
        create_user(&server.pool, email, false).await;
        let id = user_id(&server.pool, email).await;
        let s = login(&server, email, PASSWORD).await;
        let token = new_token(&server, &s).await;
        link(&server.pool, id, "attacker-sub").await;
        assert_eq!(me_with_token(&server, &token).await, 200);

        let mut tx = server.pool.begin().await.unwrap();
        users::cut_off_after_recovery(&mut tx, id).await.unwrap();
        tx.commit().await.unwrap();

        assert_eq!(me_with_token(&server, &token).await, 401);
        assert_eq!(get(&server, "/api/admin/me", &s).await.status, 401);
        assert_eq!(identities(&server.pool, id).await, 0);
    })
    .await;
}

#[tokio::test]
async fn unlink_needs_the_owner_and_keeps_a_way_to_sign_in() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        for email in ["a@example.test", "b@example.test"] {
            create_user(&server.pool, email, false).await;
        }
        let (a_id, b_id) = (
            user_id(&server.pool, "a@example.test").await,
            user_id(&server.pool, "b@example.test").await,
        );
        let a = login(&server, "a@example.test", PASSWORD).await;
        let of_a = link(&server.pool, a_id, "sub-a").await;
        let of_b = link(&server.pool, b_id, "sub-b").await;

        // 他人のものは消せない
        let path = |id: Uuid| format!("/api/admin/me/identities/{id}");
        assert_eq!(
            send(&server, "DELETE", &path(of_b), &a, None).await.status,
            404
        );
        assert_eq!(identities(&server.pool, b_id).await, 1);

        // 再認証が要る
        expire_reauth(&server.pool, "a@example.test").await;
        let res = send(&server, "DELETE", &path(of_a), &a, None).await;
        assert_eq!(
            (res.status, res.json_str("error").as_deref()),
            (403, Some("reauth_required"))
        );
        let a = login(&server, "a@example.test", PASSWORD).await;

        // パスワードがなければ、最後の1つは消せない
        sqlx::query("UPDATE yomi.users SET password_hash = NULL WHERE id = $1")
            .bind(a_id)
            .execute(&server.pool)
            .await
            .unwrap();
        let res = send(&server, "DELETE", &path(of_a), &a, None).await;
        assert_eq!(
            (res.status, res.json_str("error").as_deref()),
            (409, Some("last_sign_in_method"))
        );
        let other = link(&server.pool, a_id, "sub-a2").await;
        assert_eq!(
            send(&server, "DELETE", &path(other), &a, None).await.status,
            204
        );
        assert_eq!(identities(&server.pool, a_id).await, 1);

        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM yomi.audit_log WHERE action = 'auth.identity_unlinked'",
        )
        .fetch_one(&server.pool)
        .await
        .unwrap();
        assert_eq!(n, 1);
    })
    .await;
}

#[tokio::test]
async fn a_stopped_user_cannot_get_in() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let email = "leaver@example.test";
        create_user(&server.pool, email, false).await;
        let id = user_id(&server.pool, email).await;
        let s = login(&server, email, PASSWORD).await;
        let token = new_token(&server, &s).await;

        assert!(users::set_disabled(&server.pool, id, true).await.unwrap());
        assert!(!users::set_disabled(&server.pool, id, true).await.unwrap());
        assert_eq!(get(&server, "/api/admin/me", &s).await.status, 401);
        assert_eq!(me_with_token(&server, &token).await, 401);
        assert_eq!(login_raw(&server, email, PASSWORD, "198.18.80.1").await.status, 401);
        let (n, notified): (i64, i64) = sqlx::query_as(
            "SELECT count(*), count(*) FILTER (WHERE notify) FROM yomi.audit_log WHERE action = 'auth.user_disabled'",
        )
        .fetch_one(&server.pool)
        .await
        .unwrap();
        assert_eq!((n, notified), (1, 1));

        // 再開すると入れる。失効したトークンは戻らない
        assert!(users::set_disabled(&server.pool, id, false).await.unwrap());
        assert_eq!(login_raw(&server, email, PASSWORD, "198.18.80.2").await.status, 200);
        assert_eq!(me_with_token(&server, &token).await, 401);
    })
    .await;
}

#[tokio::test]
async fn deploy_tokens_are_separate_and_short_lived() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        create_user(&server.pool, "root@example.test", true).await;
        let root = login(&server, "root@example.test", PASSWORD).await;
        enroll(&server, &root).await;
        let create = |body: String| {
            let (server, root) = (&server, root.clone());
            async move { send(server, "POST", "/api/admin/me/tokens", &root, Some(&body)).await }
        };
        // ほかの範囲と混ぜられない
        let res = create(r#"{"scopes":["deploy","read"]}"#.to_owned()).await;
        assert_eq!((res.status, res.json_str("error").as_deref()), (422, Some("deploy_scope_exclusive")));
        // 既定の期限は30日で、それより長くはできない
        let res = create(r#"{"scopes":["deploy"]}"#.to_owned()).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let days: f64 = sqlx::query_scalar(
            "SELECT max(extract(epoch FROM expires_at - now()))::float8 / 86400 FROM yomi.personal_tokens WHERE 'deploy' = ANY(scopes)",
        )
        .fetch_one(&server.pool)
        .await
        .unwrap();
        assert!((29.0..=30.0).contains(&days), "{days}");
        let res = create(format!(r#"{{"scopes":["deploy"],"expiresAt":"{}"}}"#, in_days(&server.pool, 31).await)).await;
        assert_eq!(res.status, 400, "{}", res.body);
        let res = create(format!(r#"{{"scopes":["deploy"],"expiresAt":"{}"}}"#, in_days(&server.pool, 29).await)).await;
        assert_eq!(res.status, 201, "{}", res.body);
        // ほかの範囲は365日まで
        let res = create(format!(r#"{{"scopes":["read"],"expiresAt":"{}"}}"#, in_days(&server.pool, 300).await)).await;
        assert_eq!(res.status, 201, "{}", res.body);
    })
    .await;
}
