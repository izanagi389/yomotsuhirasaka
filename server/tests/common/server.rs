//! 結合テストの部品: テスト用のサーバーの起動と、最小限のHTTP/1.1のクライアント（依存を増やさないため）。

use std::net::SocketAddr;

use cms_server::auth::{password, users};
use cms_server::config::{Config, DatabaseConfig, LimitsConfig, SessionConfig};
use cms_server::{db, http, keys, migrate};
use sqlx::PgPool;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use totp_rs::{Builder, Secret};

pub const PASSWORD: &str = "correct horse battery staple";

pub fn config(app: &DatabaseConfig, migration: &DatabaseConfig) -> Config {
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
        smtp: None,
    }
}

pub struct Server {
    pub addr: SocketAddr,
    pub pool: PgPool,
    /// ジョブを手で1つずつ実行するための状態（テストでは常駐の処理を動かさない）。
    pub state: http::AppState,
}

pub async fn start(config: Config) -> Server {
    let migration = db::migration_pool(&config.migration_database)
        .await
        .unwrap();
    let pool = db::app_pool(&config.database).await.unwrap();
    let app_role = db::current_role(&pool).await.unwrap();
    migrate::run(&migration).await.unwrap();
    migrate::apply_grants(&migration, &app_role).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = http::AppState::new(
        pool.clone(),
        migration,
        app_role,
        config,
        keys::KeyRing::parse(&keys::generate_key_file()).unwrap(),
    );
    let app = http::router(state.clone());
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Server { addr, pool, state }
}

pub async fn create_user(pool: &PgPool, email: &str, admin: bool) {
    let hash = password::hash(PASSWORD.to_owned()).await.unwrap();
    users::create(pool, email, &hash, admin).await.unwrap();
}

#[derive(Debug)]
pub struct Resp {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Resp {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    pub fn set_cookies(&self) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
            .map(|(_, v)| v.as_str())
            .collect()
    }
    /// JSONの本文から、文字列の値を1つ取り出す（テスト用の簡易な読み取り）。
    pub fn json_str(&self, key: &str) -> Option<String> {
        let needle = format!("\"{key}\":\"");
        let start = self.body.find(&needle)? + needle.len();
        let end = self.body[start..].find('"')? + start;
        Some(self.body[start..end].to_owned())
    }
}

/// 最小限のHTTP/1.1のクライアント（依存を増やさないため）。
pub async fn request(
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
pub struct Login {
    pub cookie: String,
    pub csrf: String,
}

impl Login {
    pub fn cookie_header(&self) -> String {
        format!("__Host-cms_session={}", self.cookie)
    }
}

pub async fn login_raw(server: &Server, email: &str, password: &str, ip: &str) -> Resp {
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

pub async fn login(server: &Server, email: &str, password: &str) -> Login {
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

pub async fn get(server: &Server, path: &str, s: &Login) -> Resp {
    request(server, "GET", path, &[("Cookie", &s.cookie_header())], None).await
}

pub async fn send(
    server: &Server,
    method: &str,
    path: &str,
    s: &Login,
    body: Option<&str>,
) -> Resp {
    request(
        server,
        method,
        path,
        &[("Cookie", &s.cookie_header()), ("X-CSRF-Token", &s.csrf)],
        body,
    )
    .await
}

pub fn assert_security_headers(res: &Resp) {
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

/// 秘密（Base32）から、今の時間の区切りから `offset` だけずらした区切りのコードを作る。
pub fn code(secret: &str, offset: i64) -> String {
    let totp = Builder::new()
        .with_secret(Secret::try_from_base32(secret).unwrap())
        .build()
        .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let step = (now / 30).checked_add_signed(offset).unwrap();
    totp.generate(step * 30).to_string()
}

/// 使用済みのコードの記録を消す（同じ区切りのコードを、別の確認で使えるようにするため）。
pub async fn forget_used_steps(pool: &PgPool, email: &str) {
    sqlx::query("UPDATE cms.users SET totp_last_step = NULL WHERE email = $1")
        .bind(email)
        .execute(pool)
        .await
        .unwrap();
}

/// ログインの1段目。多要素認証が必要なら、途中のセッションのCookieを返す。
pub async fn password_step(server: &Server, email: &str, password: &str) -> (Resp, Option<String>) {
    let res = login_raw(server, email, password, "198.18.10.1").await;
    let cookie = res.set_cookies().first().map(|c| {
        c.split(';')
            .next()
            .unwrap()
            .strip_prefix("__Host-cms_session=")
            .unwrap()
            .to_owned()
    });
    (res, cookie)
}

pub async fn mfa_step(server: &Server, pending: &str, body: &str) -> Resp {
    request(
        server,
        "POST",
        "/api/admin/session/mfa",
        &[
            ("Cookie", &format!("__Host-cms_session={pending}")),
            ("X-Forwarded-For", "198.18.10.2"),
        ],
        Some(body),
    )
    .await
}

pub fn session_from(res: &Resp) -> Login {
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

/// TOTPを設定し、(秘密, 復旧コードの本文) を返す。
pub async fn enroll(server: &Server, s: &Login) -> (String, String) {
    let setup = send(server, "POST", "/api/admin/me/mfa/totp", s, None).await;
    assert_eq!(setup.status, 200, "{}", setup.body);
    let secret = setup.json_str("secret").unwrap();
    let confirm = send(
        server,
        "POST",
        "/api/admin/me/mfa/totp/confirm",
        s,
        Some(&format!("{{\"code\":\"{}\"}}", code(&secret, 0))),
    )
    .await;
    assert_eq!(confirm.status, 200, "{}", confirm.body);
    (secret, confirm.body)
}

pub fn recovery_codes(body: &str) -> Vec<String> {
    let list = body.split('[').nth(1).unwrap().split(']').next().unwrap();
    list.split(',')
        .map(|c| c.trim().trim_matches('"').to_owned())
        .collect()
}

/// 利用者のすべてのセッションの「最後の本人確認」を1時間前にする（再認証の有効時間を過ぎた状態を、待たずに作る）。
pub async fn expire_reauth(pool: &PgPool, email: &str) {
    sqlx::query(
        "UPDATE cms.sessions SET authenticated_at = now() - interval '1 hour'
         WHERE user_id = (SELECT id FROM cms.users WHERE email = $1)",
    )
    .bind(email)
    .execute(pool)
    .await
    .unwrap();
}
