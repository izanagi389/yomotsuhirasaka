//! ログイン・セッション・再認証・パスワードの変更の結合テスト（本物のPostgreSQLとHTTPを使う）。
//!
//! 環境変数 `CMS_TEST_PG_SUPERUSER_URL` があるときだけ実行する。
//! 約束そのもの（INV-AUTH-*）は tests/invariants/ が外から確かめる。ここでは部品の振る舞いを細かく確かめる。

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use cms_server::auth::{password, users};
use cms_server::config::{Config, DatabaseConfig, LimitsConfig, SessionConfig};
use cms_server::{db, http, migrate};
use common::with_scratch;
use sqlx::PgPool;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const PASSWORD: &str = "correct horse battery staple";

fn config(app: &DatabaseConfig, migration: &DatabaseConfig) -> Config {
    Config {
        production: true,
        dev_mode: false,
        public_url: "https://cms.example.test".to_owned(),
        listen: "127.0.0.1:0".to_owned(),
        log_level: "info".to_owned(),
        encryption_key_file: None,
        database: app.clone(),
        migration_database: migration.clone(),
        // テストからは X-Forwarded-For で接続元を変える
        trusted_proxies: vec!["127.0.0.1".parse().unwrap()],
        session: SessionConfig::default(),
        limits: LimitsConfig {
            login_attempts_per_ip_per_minute: 1000,
        },
    }
}

struct Server {
    addr: SocketAddr,
    pool: PgPool,
}

async fn start(config: Config) -> Server {
    let migration = db::migration_pool(&config.migration_database)
        .await
        .unwrap();
    let pool = db::app_pool(&config.database).await.unwrap();
    let app_role = db::current_role(&pool).await.unwrap();
    migrate::run(&migration).await.unwrap();
    migrate::apply_grants(&migration, &app_role).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = http::router(http::AppState::new(pool.clone(), config));
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Server { addr, pool }
}

async fn create_user(pool: &PgPool, email: &str, admin: bool) {
    let hash = password::hash(PASSWORD.to_owned()).await.unwrap();
    users::create(pool, email, &hash, admin).await.unwrap();
}

#[derive(Debug)]
struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    fn set_cookies(&self) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
            .map(|(_, v)| v.as_str())
            .collect()
    }
    /// JSONの本文から、文字列の値を1つ取り出す（テスト用の簡易な読み取り）。
    fn json_str(&self, key: &str) -> Option<String> {
        let needle = format!("\"{key}\":\"");
        let start = self.body.find(&needle)? + needle.len();
        let end = self.body[start..].find('"')? + start;
        Some(self.body[start..end].to_owned())
    }
}

/// 最小限のHTTP/1.1のクライアント（依存を増やさないため）。
async fn request(
    server: &Server,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> Resp {
    let mut stream = tokio::net::TcpStream::connect(server.addr).await.unwrap();
    let mut req =
        format!("{method} {path} HTTP/1.1\r\nHost: cms.example.test\r\nConnection: close\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(b) = body {
        req.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ));
    } else {
        req.push_str("Content-Length: 0\r\n\r\n");
    }
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let text = String::from_utf8(raw).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_owned()))
        .collect();
    assert!(
        !headers
            .iter()
            .any(|(k, v)| k == "transfer-encoding" && v.contains("chunked")),
        "the test client does not decode chunked bodies"
    );
    Resp {
        status,
        headers,
        body: body.to_owned(),
    }
}

/// ログインして得たもの。
#[derive(Clone)]
struct Login {
    cookie: String,
    csrf: String,
}

impl Login {
    fn cookie_header(&self) -> String {
        format!("__Host-cms_session={}", self.cookie)
    }
}

async fn login_raw(server: &Server, email: &str, password: &str, ip: &str) -> Resp {
    let body = format!("{{\"email\":\"{email}\",\"password\":\"{password}\"}}");
    request(
        server,
        "POST",
        "/api/admin/session",
        &[("X-Forwarded-For", ip)],
        Some(&body),
    )
    .await
}

async fn login(server: &Server, email: &str, password: &str) -> Login {
    let res = login_raw(server, email, password, "198.18.0.1").await;
    assert_eq!(res.status, 200, "{}", res.body);
    let cookie = res.set_cookies()[0]
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("__Host-cms_session=")
        .unwrap()
        .to_owned();
    Login {
        cookie,
        csrf: res.json_str("csrfToken").unwrap(),
    }
}

async fn get(server: &Server, path: &str, s: &Login) -> Resp {
    request(server, "GET", path, &[("Cookie", &s.cookie_header())], None).await
}

async fn send(server: &Server, method: &str, path: &str, s: &Login, body: Option<&str>) -> Resp {
    request(
        server,
        method,
        path,
        &[("Cookie", &s.cookie_header()), ("X-CSRF-Token", &s.csrf)],
        body,
    )
    .await
}

fn assert_security_headers(res: &Resp) {
    let csp = res.header("content-security-policy").expect("CSP");
    assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
    assert!(
        !csp.contains("unsafe-inline") && !csp.contains("unsafe-eval"),
        "{csp}"
    );
    assert!(
        res.header("strict-transport-security")
            .unwrap()
            .contains("max-age=")
    );
    assert_eq!(res.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(res.header("cache-control"), Some("no-store"));
    assert_eq!(res.header("access-control-allow-origin"), None);
}

#[tokio::test]
async fn login_me_csrf_logout() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        create_user(&server.pool, "alice@example.test", false).await;

        // ログインしていなければ使えない
        let res = request(&server, "GET", "/api/admin/me", &[], None).await;
        assert_eq!(res.status, 401);
        assert_security_headers(&res);

        // 失敗の応答は、利用者の有無で変わらない
        let wrong = login_raw(
            &server,
            "alice@example.test",
            "wrong password!!",
            "198.18.1.1",
        )
        .await;
        let unknown = login_raw(
            &server,
            "nobody@example.test",
            "wrong password!!",
            "198.18.1.2",
        )
        .await;
        assert_eq!(wrong.status, 401);
        assert_eq!((wrong.status, &wrong.body), (unknown.status, &unknown.body));
        assert!(wrong.set_cookies().is_empty());

        // ログイン: Cookieの属性
        let res = login_raw(&server, "  Alice@Example.TEST ", PASSWORD, "198.18.1.3").await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert_security_headers(&res);
        let cookie = res.set_cookies()[0].to_owned();
        assert!(cookie.starts_with("__Host-cms_session="), "{cookie}");
        for attr in ["Path=/", "Secure", "HttpOnly", "SameSite=Strict"] {
            assert!(
                cookie.split("; ").any(|a| a == attr),
                "{attr} missing: {cookie}"
            );
        }
        assert!(!cookie.to_lowercase().contains("domain="));
        assert!(!res.body.contains("argon2") && !res.body.contains(PASSWORD));
        let s = login(&server, "alice@example.test", PASSWORD).await;

        let me = get(&server, "/api/admin/me", &s).await;
        assert_eq!(me.status, 200);
        assert_eq!(me.json_str("email").as_deref(), Some("alice@example.test"));
        assert!(!me.body.contains("argon2"));
        assert_security_headers(&me);

        // CSRFトークンがない・違う・別のオリジン
        let no_csrf = request(
            &server,
            "DELETE",
            "/api/admin/session",
            &[("Cookie", &s.cookie_header())],
            None,
        )
        .await;
        assert_eq!(no_csrf.status, 403);
        let other = login(&server, "alice@example.test", PASSWORD).await;
        let wrong_csrf = request(
            &server,
            "DELETE",
            "/api/admin/session",
            &[
                ("Cookie", &s.cookie_header()),
                ("X-CSRF-Token", &other.csrf),
            ],
            None,
        )
        .await;
        assert_eq!(wrong_csrf.status, 403);
        let cross = request(
            &server,
            "DELETE",
            "/api/admin/session",
            &[
                ("Cookie", &s.cookie_header()),
                ("X-CSRF-Token", &s.csrf),
                ("Origin", "https://evil.example.test"),
            ],
            None,
        )
        .await;
        assert_eq!(cross.status, 403);
        // Bearerの経路ではCookieを読まない
        let bearer = request(
            &server,
            "GET",
            "/api/admin/me",
            &[
                ("Cookie", &s.cookie_header()),
                ("Authorization", "Bearer x"),
            ],
            None,
        )
        .await;
        assert_eq!(bearer.status, 401);
        assert_eq!(
            get(&server, "/api/admin/me", &s).await.status,
            200,
            "rejected requests must not break the session"
        );

        // ログアウト
        let out = send(&server, "DELETE", "/api/admin/session", &s, None).await;
        assert_eq!(out.status, 204);
        assert!(out.set_cookies()[0].contains("Max-Age=0"));
        assert_eq!(get(&server, "/api/admin/me", &s).await.status, 401);
        assert_eq!(get(&server, "/api/admin/me", &other).await.status, 200);

        // 前のセッションを持ったままログインすると、前のセッションは失効する
        let res = request(
            &server,
            "POST",
            "/api/admin/session",
            &[
                ("Cookie", &other.cookie_header()),
                ("X-Forwarded-For", "198.18.1.4"),
            ],
            Some(&format!(
                "{{\"email\":\"alice@example.test\",\"password\":\"{PASSWORD}\"}}"
            )),
        )
        .await;
        assert_eq!(res.status, 200);
        assert_eq!(get(&server, "/api/admin/me", &other).await.status, 401);

        // 不明なパスは404（JSON）
        let res = get(&server, "/api/admin/nothing-here", &s).await;
        assert_eq!(res.status, 404);
        assert_security_headers(&res);
    })
    .await;
}

#[tokio::test]
async fn account_throttle_and_ip_limit() {
    with_scratch(|app, migration| async move {
        let mut cfg = config(&app, &migration);
        cfg.limits.login_attempts_per_ip_per_minute = 3;
        let server = start(cfg).await;
        create_user(&server.pool, "bob@example.test", false).await;
        create_user(&server.pool, "carol@example.test", false).await;

        // 失敗が続くと、接続元を変えても、正しいパスワードで入れない
        for i in 0..5 {
            let res = login_raw(&server, "bob@example.test", "wrong password!!", &format!("198.18.2.{i}")).await;
            assert_eq!(res.status, 401);
        }
        let during = login_raw(&server, "bob@example.test", PASSWORD, "198.18.2.100").await;
        let wrong = login_raw(&server, "carol@example.test", "wrong password!!", "198.18.2.101").await;
        assert_eq!(during.status, 401);
        assert_eq!(during.body, wrong.body);
        let names = |r: &Resp| {
            let mut v: Vec<String> = r
                .headers
                .iter()
                .map(|(k, _)| k.clone())
                .filter(|k| k != "date" && k != "content-length")
                .collect();
            v.sort();
            v
        };
        assert_eq!(names(&during), names(&wrong));
        // 他の利用者には影響しない
        assert_eq!(login_raw(&server, "carol@example.test", PASSWORD, "198.18.2.102").await.status, 200);

        // 待ち時間が明ければ入れる（DBの時刻を進める代わりに、待ち時間を過去にする）
        sqlx::query("UPDATE cms.users SET login_blocked_until = now() - interval '1 second' WHERE email = 'bob@example.test'")
            .execute(&server.pool)
            .await
            .unwrap();
        assert_eq!(login_raw(&server, "bob@example.test", PASSWORD, "198.18.2.103").await.status, 200);
        // 成功すると失敗の記録は消える
        let count: i32 = sqlx::query_scalar("SELECT failed_login_count FROM cms.users WHERE email = 'bob@example.test'")
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);

        // IPアドレスごとの制限（成功・失敗を問わず数える）
        for _ in 0..3 {
            assert_ne!(login_raw(&server, "carol@example.test", PASSWORD, "198.18.3.1").await.status, 429);
        }
        let limited = login_raw(&server, "carol@example.test", PASSWORD, "198.18.3.1").await;
        assert_eq!(limited.status, 429);
        assert!(limited.header("retry-after").is_some());
        assert_eq!(login_raw(&server, "carol@example.test", PASSWORD, "198.18.3.2").await.status, 200);
    })
    .await;
}

#[tokio::test]
async fn reauth_and_password_change() {
    with_scratch(|app, migration| async move {
        let mut cfg = config(&app, &migration);
        cfg.session.reauth_window_seconds = 1;
        let server = start(cfg).await;
        create_user(&server.pool, "dave@example.test", false).await;
        let s1 = login(&server, "dave@example.test", PASSWORD).await;
        let s2 = login(&server, "dave@example.test", PASSWORD).await;
        tokio::time::sleep(Duration::from_millis(1500)).await;

        let new_password = "a brand new passphrase";
        let change = |current: &str, new: &str| {
            format!("{{\"currentPassword\":\"{current}\",\"newPassword\":\"{new}\"}}")
        };

        // 再認証の有効時間を過ぎていれば、変更できない
        let res = send(
            &server,
            "POST",
            "/api/admin/me/password",
            &s1,
            Some(&change(PASSWORD, new_password)),
        )
        .await;
        assert_eq!(res.status, 403);
        assert_eq!(res.json_str("error").as_deref(), Some("reauth_required"));

        // 誤ったパスワードでの再認証は通らない。多要素認証のコードも（まだ設定できないので）通らない
        let res = send(
            &server,
            "POST",
            "/api/admin/session/reauth",
            &s1,
            Some("{\"password\":\"wrong password!!\"}"),
        )
        .await;
        assert_eq!(res.status, 403);
        let res = send(
            &server,
            "POST",
            "/api/admin/session/reauth",
            &s1,
            Some("{\"totpCode\":\"123456\"}"),
        )
        .await;
        assert_eq!(res.status, 403);
        let res = send(
            &server,
            "POST",
            "/api/admin/session/reauth",
            &s1,
            Some(&format!("{{\"password\":\"{PASSWORD}\"}}")),
        )
        .await;
        assert_eq!(res.status, 204);

        // 弱いパスワード・誤った現在のパスワード
        let res = send(
            &server,
            "POST",
            "/api/admin/me/password",
            &s1,
            Some(&change(PASSWORD, "short pass!")),
        )
        .await;
        assert_eq!(res.status, 422);
        let res = send(
            &server,
            "POST",
            "/api/admin/me/password",
            &s1,
            Some(&change("wrong password!!", new_password)),
        )
        .await;
        assert_eq!(res.status, 403);

        // 変更すると、他のセッションは使えなくなり、変更したセッションは使える
        let res = send(
            &server,
            "POST",
            "/api/admin/me/password",
            &s1,
            Some(&change(PASSWORD, new_password)),
        )
        .await;
        assert_eq!(res.status, 204, "{}", res.body);
        assert_eq!(get(&server, "/api/admin/me", &s2).await.status, 401);
        assert_eq!(get(&server, "/api/admin/me", &s1).await.status, 200);
        assert_eq!(
            login_raw(&server, "dave@example.test", PASSWORD, "198.18.4.1")
                .await
                .status,
            401
        );
        assert_eq!(
            login_raw(&server, "dave@example.test", new_password, "198.18.4.2")
                .await
                .status,
            200
        );

        // 本文が読めないときは400で、入力の値を返さない
        let res = send(
            &server,
            "POST",
            "/api/admin/session/reauth",
            &s1,
            Some("{\"password\": 12345}"),
        )
        .await;
        assert_eq!(res.status, 400);
        assert!(!res.body.contains("12345"));
    })
    .await;
}

#[tokio::test]
async fn sessions_expire_and_can_be_listed_and_revoked() {
    with_scratch(|app, migration| async move {
        let mut cfg = config(&app, &migration);
        cfg.session.absolute_lifetime_seconds = 1;
        let short = start(cfg).await;
        create_user(&short.pool, "erin@example.test", false).await;
        let s = login(&short, "erin@example.test", PASSWORD).await;
        assert_eq!(get(&short, "/api/admin/me", &s).await.status, 200);
        tokio::time::sleep(Duration::from_millis(2000)).await;
        assert_eq!(get(&short, "/api/admin/me", &s).await.status, 401);

        let mut cfg = config(&app, &migration);
        cfg.session.idle_timeout_seconds = 1;
        let idle = start(cfg).await;
        let s = login(&idle, "erin@example.test", PASSWORD).await;
        tokio::time::sleep(Duration::from_millis(2000)).await;
        assert_eq!(get(&idle, "/api/admin/me", &s).await.status, 401);

        let server = start(config(&app, &migration)).await;
        create_user(&server.pool, "frank@example.test", false).await;
        create_user(&server.pool, "hana@example.test", false).await;
        let a = login(&server, "hana@example.test", PASSWORD).await;
        let b = login(&server, "hana@example.test", PASSWORD).await;
        let f = login(&server, "frank@example.test", PASSWORD).await;
        let list = get(&server, "/api/admin/me/sessions", &a).await;
        assert_eq!(list.status, 200);
        assert_eq!(list.body.matches("\"current\":true").count(), 1);
        assert_eq!(list.body.matches("\"id\":").count(), 2, "{}", list.body);

        // 他人のセッションは失効させられない
        let franks: String = sqlx::query_scalar(
            "SELECT s.id::text FROM cms.sessions s JOIN cms.users u ON u.id = s.user_id WHERE u.email = 'frank@example.test'",
        )
        .fetch_one(&server.pool)
        .await
        .unwrap();
        let res = send(&server, "DELETE", &format!("/api/admin/me/sessions/{franks}"), &a, None).await;
        assert_eq!(res.status, 404);
        assert_eq!(get(&server, "/api/admin/me", &f).await.status, 200);

        // 自分の別のセッションは失効させられる
        let list_b = get(&server, "/api/admin/me/sessions", &b).await;
        let b_id = list_b
            .body
            .split('{')
            .find(|o| o.contains("\"current\":true"))
            .and_then(|o| o.split("\"id\":\"").nth(1))
            .and_then(|r| r.split('"').next())
            .unwrap()
            .to_owned();
        let res = send(&server, "DELETE", &format!("/api/admin/me/sessions/{b_id}"), &a, None).await;
        assert_eq!(res.status, 204);
        assert_eq!(get(&server, "/api/admin/me", &b).await.status, 401);
        assert_eq!(get(&server, "/api/admin/me", &a).await.status, 200);
    })
    .await;
}

#[tokio::test]
async fn disabled_users_cannot_log_in_or_use_sessions() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        create_user(&server.pool, "gina@example.test", false).await;
        let s = login(&server, "gina@example.test", PASSWORD).await;
        // 管理APIができるまで、無効化はDBで直接行う（テストの準備のため）
        sqlx::query("UPDATE cms.users SET disabled = true WHERE email = 'gina@example.test'")
            .execute(&server.pool)
            .await
            .unwrap();
        assert_eq!(get(&server, "/api/admin/me", &s).await.status, 401);
        let res = login_raw(&server, "gina@example.test", PASSWORD, "198.18.5.1").await;
        let wrong = login_raw(&server, "nobody@example.test", PASSWORD, "198.18.5.2").await;
        assert_eq!((res.status, &res.body), (wrong.status, &wrong.body));
    })
    .await;
}

#[tokio::test]
async fn create_server_admin_cli() {
    with_scratch(|app, migration| async move {
        let dir =
            std::env::temp_dir().join(format!("cms-cli-test-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = dir.join("encryption.key");
        std::fs::write(&key, cms_server::keys::generate_key_file()).unwrap();
        let mut cfg = config(&app, &migration);
        cfg.trusted_proxies = Vec::new();
        cfg.encryption_key_file = Some(key);
        let path = dir.join("config.toml");
        std::fs::write(&path, toml::to_string(&cfg).unwrap()).unwrap();

        let run = |stdin: &str| {
            let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_cms-server"))
                .args(["create-server-admin", "--config"])
                .arg(&path)
                .args(["--email", "Root@Example.TEST", "--password-stdin"])
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
        let weak = run("too short\n");
        assert!(!weak.status.success());
        let out = run(&format!("{PASSWORD}\n"));
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(out.status.success(), "{stdout}\n{stderr}");
        assert!(!stdout.contains(PASSWORD) && !stderr.contains(PASSWORD));
        // 同じメールアドレスでは作れない
        assert!(!run(&format!("{PASSWORD}\n")).status.success());

        let server = start(config(&app, &migration)).await;
        let s = login(&server, "root@example.test", PASSWORD).await;
        let me = get(&server, "/api/admin/me", &s).await;
        assert!(me.body.contains("\"isServerAdmin\":true"), "{}", me.body);
        std::fs::remove_dir_all(&dir).unwrap();
    })
    .await;
}
