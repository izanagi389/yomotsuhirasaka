//! 招待と、所属者の一覧の閲覧の権限の結合テスト（本物のPostgreSQLとHTTPを使う）。
//!
//! 環境変数 `CMS_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use cms_server::worker;
use common::server::*;
use common::smtp::*;
use common::with_scratch;
use sqlx::PgPool;
use uuid::Uuid;

/// 利用者を作ってログインし、必要ならTOTPを設定する。
async fn user(server: &Server, email: &str, server_admin: bool, mfa: bool) -> (Uuid, Login) {
    create_user(&server.pool, email, server_admin).await;
    let s = login(server, email, PASSWORD).await;
    if mfa {
        enroll(server, &s).await;
    }
    let id = sqlx::query_scalar("SELECT id FROM cms.users WHERE email = $1")
        .bind(email)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    (id, s)
}

async fn add_member(pool: &PgPool, ws: Uuid, user: Uuid, role: &str) {
    sqlx::query("INSERT INTO cms.memberships (workspace_id, user_id, role) VALUES ($1, $2, $3)")
        .bind(ws)
        .bind(user)
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
}

async fn create_ws(server: &Server, s: &Login) -> Uuid {
    let res = send(
        server,
        "POST",
        "/api/admin/workspaces",
        s,
        Some("{\"name\":\"Blog\"}"),
    )
    .await;
    assert_eq!(res.status, 201, "{}", res.body);
    res.json_str("id").unwrap().parse().unwrap()
}

async fn invite(server: &Server, s: &Login, ws: Uuid, email: &str, role: &str) -> Resp {
    send(
        server,
        "POST",
        &format!("/api/admin/workspaces/{ws}/invitations"),
        s,
        Some(&format!("{{\"email\":\"{email}\",\"role\":\"{role}\"}}")),
    )
    .await
}

fn token_from(text: &str) -> String {
    let start = text.find("#token=").expect("link with token") + "#token=".len();
    text[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

async fn accept_new(server: &Server, token: &str, password: &str) -> Resp {
    request(
        server,
        "POST",
        "/api/admin/invitations/accept",
        &[("X-Forwarded-For", "198.18.20.1")],
        Some(&format!(
            "{{\"token\":\"{token}\",\"newPassword\":\"{password}\"}}"
        )),
    )
    .await
}

async fn accept_member(server: &Server, s: &Login, token: &str) -> Resp {
    send(
        server,
        "POST",
        "/api/admin/me/invitations/accept",
        s,
        Some(&format!("{{\"token\":\"{token}\"}}")),
    )
    .await
}

/// 応答のJSONのキーの一覧（値は比べない）。
fn keys(body: &str) -> Vec<String> {
    let mut out: Vec<String> = body
        .split('"')
        .collect::<Vec<_>>()
        .windows(2)
        .filter(|w| w[1].starts_with(':'))
        .map(|w| w[0].to_owned())
        .collect();
    out.sort();
    out
}

async fn audit_count(pool: &PgPool, ws: Uuid, action: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM cms.audit_log WHERE workspace_id = $1 AND action = $2")
        .bind(ws)
        .bind(action)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn invitations_without_smtp() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (_, root) = user(&server, "root@example.test", true, true).await;
        let ws = create_ws(&server, &root).await;
        let (admin_id, admin) = user(&server, "admin@example.test", false, true).await;
        add_member(&server.pool, ws, admin_id, "admin").await;
        let (_, existing) = user(&server, "existing@example.test", false, false).await;

        // SMTPがないとき、ワークスペースの管理者は招待できない（Q194）
        let res = invite(&server, &admin, ws, "new@example.test", "editor").await;
        assert_eq!((res.status, res.json_str("error").as_deref()), (403, Some("smtp_required")));

        // サーバー管理者は招待でき、リンクを一度だけ受け取る。応答は相手が既存の利用者かどうかで変わらない
        let to_new = invite(&server, &root, ws, "New@Example.test", "editor").await;
        let to_existing = invite(&server, &root, ws, "existing@example.test", "author").await;
        assert_eq!((to_new.status, to_existing.status), (201, 201), "{}", to_new.body);
        assert_eq!(keys(&to_new.body), keys(&to_existing.body));
        assert_eq!(to_new.json_str("delivery").as_deref(), Some("link"));
        let url = to_new.json_str("inviteUrl").unwrap();
        assert!(url.starts_with("https://cms.example.test/admin/invitations/accept#token="), "{url}");
        let new_token = token_from(&url);
        let existing_token = token_from(&to_existing.json_str("inviteUrl").unwrap());
        // トークンはハッシュだけを保存する
        let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM cms.invitations WHERE token_hash = convert_to($1, 'UTF8')")
            .bind(&new_token)
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert_eq!(stored, 0);
        assert_eq!(audit_count(&server.pool, ws, "member.invited").await, 2);

        // 承諾待ちの一覧
        let pending = get(&server, &format!("/api/admin/workspaces/{ws}/invitations"), &admin).await;
        assert_eq!(pending.status, 200);
        assert!(pending.body.contains("new@example.test") && pending.body.contains("existing@example.test"));
        assert!(!pending.body.contains(&new_token));

        // 新しい利用者: 弱いパスワードではトークンを使わずに断り、直せば承諾できる。二度目は使えない
        assert_eq!(accept_new(&server, &new_token, "short pass!").await.status, 422);
        for weak in ["aaaaaaaaaaaa", "abcdefghijkl", "new@example.test-2026"] {
            assert_eq!(accept_new(&server, &new_token, weak).await.status, 422, "{weak}");
        }
        let accepted = accept_new(&server, &new_token, "a long enough passphrase").await;
        assert_eq!(accepted.status, 201, "{}", accepted.body);
        assert_eq!(accepted.json_str("role").as_deref(), Some("editor"));
        let reused = accept_new(&server, &new_token, "a long enough passphrase").await;
        assert_eq!((reused.status, reused.json_str("error").as_deref()), (400, Some("invalid_token")));
        let newbie = login(&server, "new@example.test", "a long enough passphrase").await;
        assert_eq!(get(&server, &format!("/api/admin/workspaces/{ws}"), &newbie).await.status, 200);
        assert_eq!(audit_count(&server.pool, ws, "member.joined").await, 1);

        // 既存の利用者: 承諾するまで所属しない（INV-TEN-05）。新しいパスワードでは承諾できない
        assert_eq!(get(&server, &format!("/api/admin/workspaces/{ws}"), &existing).await.status, 404);
        let res = accept_new(&server, &existing_token, "a long enough passphrase").await;
        assert_eq!((res.status, res.json_str("error").as_deref()), (409, Some("account_exists")));
        // 宛先でない人のセッションでは、使えないトークンと同じ応答
        let wrong_person = accept_member(&server, &newbie, &existing_token).await;
        let garbage = accept_member(&server, &newbie, "not-a-token").await;
        assert_eq!((wrong_person.status, &wrong_person.body), (400, &garbage.body));
        let ok = accept_member(&server, &existing, &existing_token).await;
        assert_eq!(ok.status, 200, "{}", ok.body);
        assert_eq!(get(&server, &format!("/api/admin/workspaces/{ws}"), &existing).await.status, 200);

        // 所属者への招待は409
        let res = invite(&server, &root, ws, "existing@example.test", "viewer").await;
        assert_eq!(res.status, 409);

        // 招待し直すと前のリンクは使えない。取り消したリンクと期限切れのリンクも使えない
        let first = token_from(&invite(&server, &root, ws, "later@example.test", "viewer").await.json_str("inviteUrl").unwrap());
        let second = invite(&server, &root, ws, "later@example.test", "viewer").await;
        let second_token = token_from(&second.json_str("inviteUrl").unwrap());
        assert_eq!(accept_new(&server, &first, "a long enough passphrase").await.status, 400);
        let id = second.json_str("id").unwrap();
        let res = send(&server, "DELETE", &format!("/api/admin/workspaces/{ws}/invitations/{id}"), &admin, None).await;
        assert_eq!(res.status, 204);
        assert_eq!(accept_new(&server, &second_token, "a long enough passphrase").await.status, 400);
        let third = token_from(&invite(&server, &root, ws, "later@example.test", "viewer").await.json_str("inviteUrl").unwrap());
        sqlx::query("UPDATE cms.invitations SET expires_at = now() - interval '1 second' WHERE accepted_at IS NULL AND revoked_at IS NULL")
            .execute(&server.pool)
            .await
            .unwrap();
        assert_eq!(accept_new(&server, &third, "a long enough passphrase").await.status, 400);

        // 招待の期限は設定（既定30分）に従う
        let res = invite(&server, &root, ws, "fresh@example.test", "viewer").await;
        let minutes: f64 = sqlx::query_scalar("SELECT extract(epoch FROM expires_at - created_at)::float8 / 60 FROM cms.invitations WHERE id = $1::uuid")
            .bind(res.json_str("id").unwrap())
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert!((minutes - 30.0).abs() < 0.1, "{minutes}");
    })
    .await;
}

#[tokio::test]
async fn invitations_with_smtp_are_mailed() {
    with_scratch(|app, migration| async move {
        let (port, smtp) = fake_smtp().await;
        let mut cfg = config(&app, &migration);
        cfg.smtp = Some(smtp_config(port));
        let server = start(cfg).await;
        let (_, root) = user(&server, "root@example.test", true, true).await;
        let ws = create_ws(&server, &root).await;
        let (admin_id, admin) = user(&server, "admin@example.test", false, true).await;
        add_member(&server.pool, ws, admin_id, "admin").await;
        // 設定や通知のメールを先に送っておく
        while worker::run_once(&server.state).await.unwrap() {}
        let before = smtp.lock().unwrap().received.len();

        // ワークスペースの管理者も招待でき、リンクは応答に含まれない
        let res = invite(&server, &admin, ws, "invitee@example.test", "author").await;
        assert_eq!(res.status, 201, "{}", res.body);
        assert_eq!(res.json_str("delivery").as_deref(), Some("email"));
        assert!(!res.body.contains("token"));
        assert!(worker::run_once(&server.state).await.unwrap());
        let mail = smtp.lock().unwrap().received[before].clone();
        assert!(mail.contains("To: invitee@example.test"), "{mail}");
        let token = token_from(&decoded_body(&mail));
        let res = accept_new(&server, &token, "a long enough passphrase").await;
        assert_eq!(res.status, 201, "{}", res.body);
        assert_eq!(res.json_str("role").as_deref(), Some("author"));
    })
    .await;
}

#[tokio::test]
async fn member_list_viewing_can_be_granted() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (_, root) = user(&server, "root@example.test", true, true).await;
        let ws = create_ws(&server, &root).await;
        let (admin_id, admin) = user(&server, "admin@example.test", false, true).await;
        let (editor_id, editor) = user(&server, "editor@example.test", false, false).await;
        add_member(&server.pool, ws, admin_id, "admin").await;
        add_member(&server.pool, ws, editor_id, "editor").await;
        let members = format!("/api/admin/workspaces/{ws}/members");
        let editor_path = format!("{members}/{editor_id}");

        assert_eq!(get(&server, &members, &editor).await.status, 403);
        let res = send(
            &server,
            "PATCH",
            &editor_path,
            &admin,
            Some("{\"canViewMembers\":true}"),
        )
        .await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert!(
            res.body.contains("\"canViewMembers\":true")
                && res.body.contains("\"role\":\"editor\"")
        );
        assert_eq!(
            audit_count(&server.pool, ws, "member.permission_changed").await,
            1
        );
        assert_eq!(
            audit_count(&server.pool, ws, "member.role_changed").await,
            0
        );

        // 閲覧の権限を与えられた人は、一覧と1人分を見られるが、変更はできない
        let list = get(&server, &members, &editor).await;
        assert_eq!(list.status, 200);
        assert!(list.body.contains("admin@example.test"));
        assert_eq!(
            get(&server, &format!("{members}/{admin_id}"), &editor)
                .await
                .status,
            200
        );
        assert_eq!(
            send(
                &server,
                "PATCH",
                &editor_path,
                &editor,
                Some("{\"role\":\"admin\"}")
            )
            .await
            .status,
            403
        );

        // 取り上げると見られない。空の変更は400
        assert_eq!(
            send(
                &server,
                "PATCH",
                &editor_path,
                &admin,
                Some("{\"canViewMembers\":false}")
            )
            .await
            .status,
            200
        );
        assert_eq!(get(&server, &members, &editor).await.status, 403);
        assert_eq!(
            send(&server, "PATCH", &editor_path, &admin, Some("{}"))
                .await
                .status,
            400
        );
    })
    .await;
}
