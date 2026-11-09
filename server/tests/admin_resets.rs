//! パスワードと多要素認証のリセットの結合テスト（本物のPostgreSQLとHTTPを使う）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::server::*;
use common::smtp::*;
use common::with_scratch;
use sqlx::PgPool;
use uuid::Uuid;
use yomi_server::worker;

const NEW_PASSWORD: &str = "a fresh long passphrase";

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

async fn post_anon(server: &Server, path: &str, body: &str) -> Resp {
    request(
        server,
        "POST",
        path,
        &[("X-Forwarded-For", "198.18.40.1")],
        Some(body),
    )
    .await
}

async fn request_reset(server: &Server, email: &str) -> Resp {
    post_anon(
        server,
        "/api/admin/password-resets",
        &format!("{{\"email\":\"{email}\"}}"),
    )
    .await
}

async fn complete_password(server: &Server, token: &str, password: &str) -> Resp {
    post_anon(
        server,
        "/api/admin/password-resets/complete",
        &format!("{{\"token\":\"{token}\",\"newPassword\":\"{password}\"}}"),
    )
    .await
}

async fn complete_mfa(server: &Server, token: &str) -> Resp {
    post_anon(
        server,
        "/api/admin/mfa-resets/complete",
        &format!("{{\"token\":\"{token}\"}}"),
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

async fn job_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM yomi.jobs")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn audit_count(pool: &PgPool, ws: Option<Uuid>, action: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM yomi.audit_log WHERE workspace_id IS NOT DISTINCT FROM $1 AND action = $2",
    )
    .bind(ws)
    .bind(action)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn self_service_reset_by_email() {
    with_scratch(|app, migration| async move {
        let (port, smtp) = fake_smtp().await;
        let mut cfg = config(&app, &migration);
        cfg.smtp = Some(smtp_config(port));
        let server = start(cfg).await;
        let (_, old_session) = user(&server, "forgot@example.test", false, false).await;
        user(&server, "root@example.test", true, false).await;
        while worker::run_once(&server.state).await.unwrap() {}
        let before = job_count(&server.pool).await;

        // 応答は登録の有無で変わらない（INV-AUTH-16）
        let registered = request_reset(&server, "Forgot@Example.test").await;
        let unregistered = request_reset(&server, "nobody@example.test").await;
        assert_eq!(registered.status, 202);
        assert_eq!((registered.status, &registered.body), (unregistered.status, &unregistered.body));
        // 登録済みのアドレスにだけメールが積まれる。直後の再要求では積まない（連打の防止）
        assert_eq!(job_count(&server.pool).await, before + 1);
        let again = request_reset(&server, "forgot@example.test").await;
        assert_eq!((again.status, &again.body), (registered.status, &registered.body));
        assert_eq!(job_count(&server.pool).await, before + 1);
        // サーバー管理者には送らない（INV-AUTH-10）
        assert_eq!(request_reset(&server, "root@example.test").await.status, 202);
        assert_eq!(job_count(&server.pool).await, before + 1);

        assert!(worker::run_once(&server.state).await.unwrap());
        let mail = smtp.lock().unwrap().received.last().unwrap().clone();
        assert!(mail.contains("To: forgot@example.test"), "{mail}");
        let body = decoded_body(&mail);
        assert!(body.contains("https://yomi.example.test/admin/reset-password#token="), "{body}");
        let token = token_from(&body);

        // 弱いパスワードではトークンを使わずに断る
        assert_eq!(complete_password(&server, &token, "qwertyuiop12").await.status, 422);
        // 完了すると、セッションはすべて失効し、新しいパスワードで入れる
        assert_eq!(complete_password(&server, &token, NEW_PASSWORD).await.status, 204);
        assert_eq!(get(&server, "/api/admin/me", &old_session).await.status, 401);
        assert_eq!(login_raw(&server, "forgot@example.test", PASSWORD, "198.18.40.2").await.status, 401);
        login(&server, "forgot@example.test", NEW_PASSWORD).await;
        // 二度目は使えない（INV-AUTH-15）
        let reused = complete_password(&server, &token, "yet another passphrase").await;
        assert_eq!((reused.status, reused.json_str("error").as_deref()), (400, Some("invalid_token")));
        assert_eq!(audit_count(&server.pool, None, "password_reset.completed").await, 1);
        // パスワードの変更の通知と、新しい端末からのログインの通知（テストのクライアントは端末のCookieを持たない）が積まれる
        assert!(worker::run_once(&server.state).await.unwrap());
        assert!(worker::run_once(&server.state).await.unwrap());
        assert!(!worker::run_once(&server.state).await.unwrap());

        // 期限切れのトークンは使えない
        sqlx::query("UPDATE yomi.reset_tokens SET created_at = now() - interval '2 minutes'")
            .execute(&server.pool)
            .await
            .unwrap();
        request_reset(&server, "forgot@example.test").await;
        assert!(worker::run_once(&server.state).await.unwrap());
        let mail = smtp.lock().unwrap().received.last().unwrap().clone();
        let expired = token_from(&decoded_body(&mail));
        sqlx::query("UPDATE yomi.reset_tokens SET expires_at = now() - interval '1 second' WHERE used_at IS NULL")
            .execute(&server.pool)
            .await
            .unwrap();
        assert_eq!(complete_password(&server, &expired, "yet another passphrase").await.status, 400);
    })
    .await;
}

#[tokio::test]
async fn self_service_reset_needs_smtp() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        user(&server, "nosmtp@example.test", false, false).await;
        let res = request_reset(&server, "nosmtp@example.test").await;
        assert_eq!(res.status, 202);
        assert_eq!(job_count(&server.pool).await, 0);
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM yomi.reset_tokens")
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "no token should be issued without a way to deliver it"
        );
    })
    .await;
}

#[tokio::test]
async fn admins_issue_resets_within_their_scope() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (_, root) = user(&server, "root@example.test", true, true).await;
        let a = create_ws(&server, &root, "A").await;
        let b = create_ws(&server, &root, "B").await;
        let (admin_id, admin) = user(&server, "admin-a@example.test", false, true).await;
        let (only_a, only_a_session) = user(&server, "only-a@example.test", false, true).await;
        let (also_b, _) = user(&server, "also-b@example.test", false, false).await;
        let (other_root, _) = user(&server, "root2@example.test", true, true).await;
        let (outsider, _) = user(&server, "outsider@example.test", false, false).await;
        add_member(&server.pool, a, admin_id, "admin").await;
        add_member(&server.pool, a, only_a, "editor").await;
        add_member(&server.pool, a, also_b, "editor").await;
        add_member(&server.pool, b, also_b, "viewer").await;
        add_member(&server.pool, a, other_root, "editor").await;
        let in_a =
            |user: Uuid, kind: &str| format!("/api/admin/workspaces/{a}/members/{user}/{kind}");

        // 他のワークスペースにも所属する人には発行できない（INV-AUTH-14）
        let res = send(
            &server,
            "POST",
            &in_a(also_b, "password-reset"),
            &admin,
            None,
        )
        .await;
        assert_eq!(
            (res.status, res.json_str("error").as_deref()),
            (403, Some("outside_scope"))
        );
        assert_eq!(
            send(&server, "POST", &in_a(also_b, "mfa-reset"), &admin, None)
                .await
                .status,
            403
        );
        // サーバー管理者には、どの経路でも発行できない（INV-AUTH-10）
        assert_eq!(
            send(
                &server,
                "POST",
                &in_a(other_root, "password-reset"),
                &admin,
                None
            )
            .await
            .status,
            403
        );
        let res = send(
            &server,
            "POST",
            &format!("/api/admin/users/{other_root}/mfa-reset"),
            &root,
            None,
        )
        .await;
        assert_eq!(
            (res.status, res.json_str("error").as_deref()),
            (403, Some("server_admin_target"))
        );
        // 所属していない人は、存在しないものと同じ
        let res = send(
            &server,
            "POST",
            &in_a(outsider, "password-reset"),
            &admin,
            None,
        )
        .await;
        let nobody = send(
            &server,
            "POST",
            &in_a(Uuid::new_v4(), "password-reset"),
            &admin,
            None,
        )
        .await;
        assert_eq!((res.status, &res.body), (404, &nobody.body));
        // ワークスペースの管理者はインスタンスの経路を使えない
        assert_eq!(
            send(
                &server,
                "POST",
                &format!("/api/admin/users/{only_a}/password-reset"),
                &admin,
                None
            )
            .await
            .status,
            403
        );

        // 対照: Aにしか所属していない人には発行でき、SMTPがなければリンクを受け取る
        let res = send(
            &server,
            "POST",
            &in_a(only_a, "password-reset"),
            &admin,
            None,
        )
        .await;
        assert_eq!(res.status, 201, "{}", res.body);
        assert_eq!(res.json_str("delivery").as_deref(), Some("link"));
        let password_token = token_from(&res.json_str("resetUrl").unwrap());
        assert_eq!(
            audit_count(&server.pool, Some(a), "password_reset.issued").await,
            1
        );
        // 再認証の有効時間を過ぎていれば発行できない
        expire_reauth(&server.pool, "admin-a@example.test").await;
        assert_eq!(
            send(&server, "POST", &in_a(only_a, "mfa-reset"), &admin, None)
                .await
                .status,
            403
        );
        // サーバー管理者は、所属していないワークスペースの経路でも、インスタンスの経路でも発行できる
        let res = send(&server, "POST", &in_a(only_a, "mfa-reset"), &root, None).await;
        assert_eq!(res.status, 201, "{}", res.body);
        assert_eq!(
            audit_count(&server.pool, Some(a), "mfa_reset.issued").await,
            1
        );
        let mfa_token = token_from(&res.json_str("resetUrl").unwrap());
        let res = send(
            &server,
            "POST",
            &format!("/api/admin/users/{also_b}/password-reset"),
            &root,
            None,
        )
        .await;
        assert_eq!(res.status, 201);

        // 種類の違うトークンは使えない
        assert_eq!(complete_mfa(&server, &password_token).await.status, 400);
        assert_eq!(
            complete_password(&server, &mfa_token, NEW_PASSWORD)
                .await
                .status,
            400
        );

        // 多要素認証のリセット: 解除され、セッションは失効し、パスワードだけでログインできる
        assert_eq!(complete_mfa(&server, &mfa_token).await.status, 204);
        assert_eq!(
            get(&server, "/api/admin/me", &only_a_session).await.status,
            401
        );
        let res = login_raw(&server, "only-a@example.test", PASSWORD, "198.18.40.3").await;
        assert!(res.body.contains("csrfToken"), "{}", res.body);
        assert_eq!(complete_mfa(&server, &mfa_token).await.status, 400);

        // 発行の後にサーバー管理者になった人のトークンは使えない
        sqlx::query("UPDATE yomi.users SET is_server_admin = true WHERE id = $1")
            .bind(only_a)
            .execute(&server.pool)
            .await
            .unwrap();
        assert_eq!(
            complete_password(&server, &password_token, NEW_PASSWORD)
                .await
                .status,
            400
        );
    })
    .await;
}

#[tokio::test]
async fn server_admins_recover_with_the_cli() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (_, root) = user(&server, "root@example.test", true, true).await;
        let dir = std::env::temp_dir().join(format!("yomi-cli-reset-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = dir.join("encryption.key");
        std::fs::write(&key, yomi_server::keys::generate_key_file()).unwrap();
        let mut cfg = config(&app, &migration);
        cfg.trusted_proxies = Vec::new();
        cfg.encryption_key_file = Some(key);
        let path = dir.join("config.toml");
        std::fs::write(&path, toml::to_string(&cfg).unwrap()).unwrap();
        let run = |args: &[&str], stdin: &str| {
            let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_yomi-server"))
                .args(args)
                .arg("--config")
                .arg(&path)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            use std::io::Write as _;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(stdin.as_bytes())
                .unwrap();
            child.wait_with_output().unwrap()
        };

        let out = run(&["reset-mfa", "--email", "root@example.test"], "");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(get(&server, "/api/admin/me", &root).await.status, 401);
        let weak = run(
            &[
                "reset-password",
                "--email",
                "root@example.test",
                "--password-stdin",
            ],
            "aaaaaaaaaaaaaa\n",
        );
        assert!(!weak.status.success());
        let out = run(
            &[
                "reset-password",
                "--email",
                "root@example.test",
                "--password-stdin",
            ],
            &format!("{NEW_PASSWORD}\n"),
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!String::from_utf8_lossy(&out.stdout).contains(NEW_PASSWORD));
        let res = login_raw(&server, "root@example.test", NEW_PASSWORD, "198.18.40.4").await;
        assert!(
            res.body.contains("csrfToken"),
            "password only after the MFA reset: {}",
            res.body
        );
        assert!(
            !run(&["reset-mfa", "--email", "nobody@example.test"], "")
                .status
                .success()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    })
    .await;
}

/// 本人の要求を重ねても、先に届いたリンクは無効にならない。どれかを使えば残りは使えない（R28）。
#[tokio::test]
async fn later_requests_do_not_cancel_a_link_already_sent() {
    with_scratch(|app, migration| async move {
        let (port, smtp) = fake_smtp().await;
        let mut cfg = config(&app, &migration);
        cfg.smtp = Some(smtp_config(port));
        let server = start(cfg).await;
        user(&server, "forgot@example.test", false, false).await;
        while worker::run_once(&server.state).await.unwrap() {}
        let mut tokens = Vec::new();
        for _ in 0..2 {
            assert_eq!(
                request_reset(&server, "forgot@example.test").await.status,
                202
            );
            while worker::run_once(&server.state).await.unwrap() {}
            let mail = smtp.lock().unwrap().received.last().unwrap().clone();
            tokens.push(token_from(&decoded_body(&mail)));
            // 1分に1通の制限を越えて、次の要求を通す
            sqlx::query(
                "UPDATE yomi.reset_tokens SET created_at = created_at - interval '2 minutes'",
            )
            .execute(&server.pool)
            .await
            .unwrap();
        }
        assert_ne!(tokens[0], tokens[1]);
        let res = complete_password(&server, &tokens[0], "a-brand-new-long-passphrase-1").await;
        assert!(res.status < 300, "{}", res.body);
        let res = complete_password(&server, &tokens[1], "a-brand-new-long-passphrase-2").await;
        assert!(res.status >= 400, "{}", res.body);
    })
    .await;
}
