//! 新しい端末の通知とログインの履歴の結合テスト（本物のPostgreSQLとHTTPを使う）。
//!
//! 環境変数 `CMS_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use cms_server::auth::devices;
use cms_server::worker;
use common::server::*;
use common::smtp::*;
use common::with_scratch;

const CHROME: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36";

/// ログインして、(応答, 端末のCookieの値) を返す。
async fn login_with(
    server: &Server,
    email: &str,
    ip: &str,
    device: Option<&str>,
) -> (Resp, String) {
    let body = format!("{{\"email\":\"{email}\",\"password\":\"{PASSWORD}\"}}");
    let cookie = device.map(|d| format!("__Host-cms_device={d}"));
    let mut headers = vec![("X-Forwarded-For", ip), ("User-Agent", CHROME)];
    if let Some(c) = &cookie {
        headers.push(("Cookie", c));
    }
    let res = request(server, "POST", "/api/admin/session", &headers, Some(&body)).await;
    assert_eq!(res.status, 200, "{}", res.body);
    let set = res
        .set_cookies()
        .into_iter()
        .find(|c| c.starts_with("__Host-cms_device="))
        .expect("device cookie")
        .to_owned();
    assert!(
        set.contains("HttpOnly")
            && set.contains("Secure")
            && set.contains("SameSite=Strict")
            && set.contains("Path=/"),
        "{set}"
    );
    let value = set
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("__Host-cms_device=")
        .unwrap()
        .to_owned();
    (res, value)
}

async fn drain(server: &Server) -> usize {
    let mut n = 0;
    while worker::run_once(&server.state).await.unwrap() {
        n += 1;
    }
    n
}

#[tokio::test]
async fn new_device_notification_and_login_history() {
    with_scratch(|app, migration| async move {
        let (port, smtp) = fake_smtp().await;
        let mut cfg = config(&app, &migration);
        cfg.smtp = Some(smtp_config(port));
        let server = start(cfg).await;
        let email = "user@example.test";
        create_user(&server.pool, email, false).await;

        // 初めての端末: 知らせない
        let (first, device) = login_with(&server, email, "198.18.1.1", None).await;
        assert_eq!(drain(&server).await, 0);

        // 同じ端末（Cookieあり）: 知らせない。Cookieの値は変わらない
        let (_, again) = login_with(&server, email, "198.18.1.2", Some(&device)).await;
        assert_eq!(again, device);
        assert_eq!(drain(&server).await, 0);

        // 新しい端末（Cookieなし）: 知らせる。秘密の値は本文に含めない（INV-AUD-06）
        let (other, other_device) = login_with(&server, email, "198.18.1.3", None).await;
        assert_ne!(other_device, device);
        assert_eq!(drain(&server).await, 1);
        let mail = smtp.lock().unwrap().received.last().unwrap().clone();
        let body = decoded_body(&mail);
        assert!(body.contains("new device") && body.contains("198.18.1.3") && body.contains("Chrome"), "{body}");
        for secret in [PASSWORD.to_owned(), device.clone(), other_device.clone(), session_from(&first).cookie, session_from(&other).cookie] {
            assert!(!mail.contains(&secret) && !body.contains(&secret));
        }

        // 形の崩れたCookieは使わず、新しい端末として扱う
        let (_, replaced) = login_with(&server, email, "198.18.1.4", Some("not-a-device-id")).await;
        assert_ne!(replaced, "not-a-device-id");
        assert_eq!(drain(&server).await, 1);

        // 別の利用者にとっては、同じ端末でも新しい端末（ただし初めての端末なので知らせない）
        create_user(&server.pool, "other@example.test", false).await;
        login_with(&server, "other@example.test", "198.18.1.5", Some(&device)).await;
        assert_eq!(drain(&server).await, 0);

        // 履歴: 本人の分だけ、新しい順。User-Agent はそのまま保存しない
        let session = session_from(&login_with(&server, email, "198.18.1.6", Some(&device)).await.0);
        let res = get(&server, "/api/admin/me/login-history", &session).await;
        assert_eq!(res.status, 200, "{}", res.body);
        for ip in ["198.18.1.1", "198.18.1.3", "198.18.1.6"] {
            assert!(res.body.contains(ip), "{}", res.body);
        }
        assert!(!res.body.contains("198.18.1.5") && !res.body.contains("Mozilla"), "{}", res.body);
        assert!(res.body.find("198.18.1.6").unwrap() < res.body.find("198.18.1.1").unwrap());
        assert!(res.body.contains("\"browser\":\"Chrome\"") && res.body.contains("\"newDevice\":true") && res.body.contains("\"country\":null"));

        // 保存期間を過ぎた履歴は見せず、削除する（INV-AUD-07）
        sqlx::query("UPDATE cms.login_history SET created_at = now() - interval '91 days' WHERE host(ip) = '198.18.1.1'")
            // 履歴は普段用のロールからは書き換えられないので、移行用の接続で古くする
            .execute(&server.state.migration)
            .await
            .unwrap();
        let res = get(&server, "/api/admin/me/login-history", &session).await;
        assert!(!res.body.contains("198.18.1.1") && res.body.contains("198.18.1.3"), "{}", res.body);
        let removed = devices::sweep(&server.pool, server.state.config.login_history.retention_seconds).await.unwrap();
        assert_eq!(removed, 1);
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM cms.login_history WHERE host(ip) = '198.18.1.1'")
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert_eq!(left, 0);

        // 長く使われていない端末の記録も消える（次のログインは新しい端末になる）
        sqlx::query("UPDATE cms.known_devices SET last_seen_at = now() - interval '401 days'")
            .execute(&server.pool)
            .await
            .unwrap();
        devices::sweep(&server.pool, server.state.config.login_history.retention_seconds).await.unwrap();
        let known: i64 = sqlx::query_scalar("SELECT count(*) FROM cms.known_devices").fetch_one(&server.pool).await.unwrap();
        assert_eq!(known, 0);

        // 個人トークンでは読めない（Cookieのセッションだけ）
        let res = request(&server, "GET", "/api/admin/me/login-history", &[("Authorization", "Bearer cms_pat_x")], None).await;
        assert_eq!(res.status, 401);
    })
    .await;
}

#[tokio::test]
async fn device_cookie_does_not_skip_mfa() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let email = "mfa@example.test";
        create_user(&server.pool, email, false).await;
        let s = login(&server, email, PASSWORD).await;
        let (secret, _) = enroll(&server, &s).await;
        forget_used_steps(&server.pool, email).await;

        // 端末のCookieは、多要素認証まで済ませた後にだけ渡る
        let (res, pending) = password_step(&server, email, PASSWORD).await;
        assert!(
            res.set_cookies()
                .iter()
                .all(|c| !c.starts_with("__Host-cms_device="))
        );
        let res = mfa_step(
            &server,
            &pending.unwrap(),
            &format!("{{\"totpCode\":\"{}\"}}", code(&secret, 0)),
        )
        .await;
        assert_eq!(res.status, 200, "{}", res.body);
        let device = res
            .set_cookies()
            .into_iter()
            .find_map(|c| {
                c.split(';')
                    .next()
                    .unwrap()
                    .strip_prefix("__Host-cms_device=")
                    .map(str::to_owned)
            })
            .expect("device cookie");

        // 知っている端末のCookieを持っていても、パスワードだけではログインできない（Q181、INV-AUTH-24）
        let body = format!("{{\"email\":\"{email}\",\"password\":\"{PASSWORD}\"}}");
        let res = request(
            &server,
            "POST",
            "/api/admin/session",
            &[
                ("X-Forwarded-For", "198.18.2.1"),
                ("Cookie", &format!("__Host-cms_device={device}")),
            ],
            Some(&body),
        )
        .await;
        assert_eq!(res.status, 200);
        assert!(res.body.contains("\"mfaRequired\":true"), "{}", res.body);
        assert!(res.json_str("csrfToken").is_none());
    })
    .await;
}

#[tokio::test]
async fn country_change_with_geoip_data() {
    // 国のデータそのものの確認
    let path = common::mmdb::write(&[
        ("198.18.1.0".parse().unwrap(), 24, "JP"),
        ("198.18.2.0".parse().unwrap(), 24, "US"),
        ("198.18.4.0".parse().unwrap(), 24, "us\nX"),
    ]);
    let geo = cms_server::geoip::GeoIp::open(&path).unwrap();
    assert_eq!(
        geo.country("198.18.1.9".parse().unwrap()).as_deref(),
        Some("JP")
    );
    assert_eq!(
        geo.country("::ffff:198.18.2.9".parse().unwrap()).as_deref(),
        Some("US")
    );
    assert_eq!(geo.country("198.18.3.1".parse().unwrap()), None);
    // IPv4だけのデータでIPv6を調べても、エラーにせず「分からない」
    assert_eq!(geo.country("2001:db8::1".parse().unwrap()), None);
    // 決まった形でない値は使わない（メールの本文に入るため）
    assert_eq!(geo.country("198.18.4.1".parse().unwrap()), None);

    // 読めないデータは起動の検査で止める
    let broken = std::env::temp_dir().join(format!(
        "cms-test-geoip-broken-{}.mmdb",
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&broken, b"not a database").unwrap();
    assert!(cms_server::geoip::GeoIp::open(&broken).is_err());

    let (db, broken_db) = (path.clone(), broken.clone());
    with_scratch(|app, migration| async move {
        let (port, smtp) = fake_smtp().await;
        let mut cfg = config(&app, &migration);
        cfg.smtp = Some(smtp_config(port));
        cfg.login_history.geoip_database = Some(db.clone());
        let report = cms_server::startup::check_config(&cfg);
        assert!(
            !report.problems.iter().any(|p| p.message.contains("geoip")),
            "{:?}",
            report
                .problems
                .iter()
                .map(|p| &p.message)
                .collect::<Vec<_>>()
        );
        let mut bad = cfg.clone();
        bad.login_history.geoip_database = Some(broken_db.clone());
        assert!(
            cms_server::startup::check_config(&bad)
                .problems
                .iter()
                .any(|p| p.message.contains("login_history.geoip_database"))
        );

        let server = start(cfg).await;
        let email = "traveler@example.test";
        create_user(&server.pool, email, false).await;
        let last_body = || decoded_body(smtp.lock().unwrap().received.last().unwrap());

        let (_, device) = login_with(&server, email, "198.18.1.1", None).await;
        login_with(&server, email, "198.18.1.2", Some(&device)).await;
        assert_eq!(drain(&server).await, 0);

        // 知っている端末でも、前回と違う国なら知らせる
        login_with(&server, email, "198.18.2.1", Some(&device)).await;
        assert_eq!(drain(&server).await, 1);
        let body = last_body();
        assert!(
            body.contains("different country")
                && body.contains("Country: US")
                && body.contains("198.18.2.1"),
            "{body}"
        );

        // 同じ国、国の分からない接続元では知らせない。比べる相手は、国の分かっている直前のログイン
        login_with(&server, email, "198.18.2.2", Some(&device)).await;
        login_with(&server, email, "198.18.3.1", Some(&device)).await;
        assert_eq!(drain(&server).await, 0);
        login_with(&server, email, "198.18.1.3", Some(&device)).await;
        assert_eq!(drain(&server).await, 1);
        assert!(last_body().contains("Country: JP"));

        // 新しい端末なら、その通知1通に国も添える
        login_with(&server, email, "198.18.2.3", None).await;
        assert_eq!(drain(&server).await, 1);
        let body = last_body();
        assert!(
            body.contains("new device") && body.contains("Country: US"),
            "{body}"
        );

        // 履歴に国が残る
        let session = session_from(
            &login_with(&server, email, "198.18.1.4", Some(&device))
                .await
                .0,
        );
        let res = get(&server, "/api/admin/me/login-history", &session).await;
        assert!(
            res.body.contains("\"country\":\"JP\"")
                && res.body.contains("\"country\":\"US\"")
                && res.body.contains("\"country\":null"),
            "{}",
            res.body
        );
    })
    .await;
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&broken);
}
