//! メールの送信とジョブのキューの結合テスト（本物のPostgreSQLと、テストの中の擬似SMTPサーバー）。
//!
//! 環境変数 `CMS_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use std::sync::{Arc, Mutex};

use cms_server::config::{SmtpConfig, SmtpSecurity};
use cms_server::worker;
use common::server::*;
use common::with_scratch;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

/// 擬似SMTPサーバーのふるまい。
#[derive(Default)]
struct Smtp {
    /// 受け取ったメール（DATAの中身）。
    received: Vec<String>,
    /// この回数だけ、宛先の段階で一時的な失敗（451）を返す。
    transient_failures: u32,
    /// 宛先の段階で恒久的な失敗（550）を返す。
    reject: bool,
}

async fn fake_smtp() -> (u16, Arc<Mutex<Smtp>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = Arc::new(Mutex::new(Smtp::default()));
    let shared = state.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let state = shared.clone();
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                write.write_all(b"220 fake ESMTP\r\n").await.unwrap();
                let mut data: Option<String> = None;
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Some(buf) = data.as_mut() {
                        if line == "." {
                            state.lock().unwrap().received.push(data.take().unwrap());
                            write.write_all(b"250 queued\r\n").await.unwrap();
                        } else {
                            buf.push_str(&line);
                            buf.push('\n');
                        }
                        continue;
                    }
                    let upper = line.to_ascii_uppercase();
                    let reply: &[u8] = if upper.starts_with("EHLO") || upper.starts_with("HELO") {
                        b"250 fake\r\n"
                    } else if upper.starts_with("RCPT") {
                        let mut s = state.lock().unwrap();
                        if s.reject {
                            b"550 5.1.1 no such user\r\n"
                        } else if s.transient_failures > 0 {
                            s.transient_failures -= 1;
                            b"451 4.3.0 try again later\r\n"
                        } else {
                            b"250 ok\r\n"
                        }
                    } else if upper.starts_with("DATA") {
                        data = Some(String::new());
                        b"354 go ahead\r\n"
                    } else if upper.starts_with("QUIT") {
                        write.write_all(b"221 bye\r\n").await.unwrap();
                        break;
                    } else {
                        b"250 ok\r\n"
                    };
                    write.write_all(reply).await.unwrap();
                }
            });
        }
    });
    (port, state)
}

fn smtp_config(port: u16) -> SmtpConfig {
    SmtpConfig {
        host: "127.0.0.1".into(),
        port: Some(port),
        security: SmtpSecurity::None,
        username: None,
        password: None,
        from: "CMS <cms@example.test>".into(),
    }
}

async fn job_rows(server: &Server) -> Vec<(i32, Option<String>, bool, bool)> {
    sqlx::query_as(
        "SELECT attempts, last_error, failed_at IS NOT NULL, payload_enc IS NOT NULL FROM cms.jobs ORDER BY created_at",
    )
    .fetch_all(&server.pool)
    .await
    .unwrap()
}

async fn change_password(server: &Server, s: &Login, from: &str, to: &str) {
    let res = send(
        server,
        "POST",
        "/api/admin/me/password",
        s,
        Some(&format!(
            "{{\"currentPassword\":\"{from}\",\"newPassword\":\"{to}\"}}"
        )),
    )
    .await;
    assert_eq!(res.status, 204, "{}", res.body);
}

#[tokio::test]
async fn notifications_are_queued_encrypted_and_retried() {
    with_scratch(|app, migration| async move {
        let (port, smtp) = fake_smtp().await;
        let mut cfg = config(&app, &migration);
        cfg.smtp = Some(smtp_config(port));
        let server = start(cfg).await;
        let email = "notify@example.test";
        create_user(&server.pool, email, false).await;
        let s = login(&server, email, PASSWORD).await;

        // パスワードの変更で通知が積まれる。中身は暗号化されている
        let new_password = "another long passphrase";
        change_password(&server, &s, PASSWORD, new_password).await;
        let stored: Vec<u8> = sqlx::query_scalar("SELECT payload_enc FROM cms.jobs")
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert!(!stored.windows(email.len()).any(|w| w == email.as_bytes()));
        assert!(
            smtp.lock().unwrap().received.is_empty(),
            "must not send inside the request"
        );

        // ジョブを実行すると届き、ジョブは消える。本文にパスワードは含まれない
        assert!(worker::run_once(&server.state).await.unwrap());
        assert!(!worker::run_once(&server.state).await.unwrap());
        let received = smtp.lock().unwrap().received.clone();
        assert_eq!(received.len(), 1);
        assert!(
            received[0].contains(&format!("To: {email}")),
            "{}",
            received[0]
        );
        assert!(received[0].contains("From: CMS <cms@example.test>"));
        assert!(!received[0].contains(new_password) && !received[0].contains(PASSWORD));
        assert!(job_rows(&server).await.is_empty());

        // 一時的な失敗はやり直す（すぐにはやり直さない）
        smtp.lock().unwrap().transient_failures = 1;
        let (secret, _) = enroll(&server, &s).await;
        assert!(worker::run_once(&server.state).await.unwrap());
        assert_eq!(
            job_rows(&server).await,
            vec![(1, Some("smtp_transient_error".to_owned()), false, true)]
        );
        assert!(
            !worker::run_once(&server.state).await.unwrap(),
            "must wait before retrying"
        );
        sqlx::query("UPDATE cms.jobs SET run_at = now()")
            .execute(&server.pool)
            .await
            .unwrap();
        assert!(worker::run_once(&server.state).await.unwrap());
        assert!(job_rows(&server).await.is_empty());
        assert_eq!(smtp.lock().unwrap().received.len(), 2);
        assert!(
            !smtp.lock().unwrap().received[1].contains(&secret),
            "the TOTP secret must not be mailed"
        );

        // 恒久的な失敗はやり直さず、中身を消して記録だけを残す
        smtp.lock().unwrap().reject = true;
        forget_used_steps(&server.pool, email).await;
        let res = send(
            &server,
            "POST",
            "/api/admin/session/reauth",
            &s,
            Some(&format!("{{\"totpCode\":\"{}\"}}", code(&secret, 0))),
        )
        .await;
        assert_eq!(res.status, 204);
        assert_eq!(
            send(&server, "DELETE", "/api/admin/me/mfa/totp", &s, None)
                .await
                .status,
            204
        );
        assert!(worker::run_once(&server.state).await.unwrap());
        assert_eq!(
            job_rows(&server).await,
            vec![(1, Some("smtp_permanent_error".to_owned()), true, false)]
        );
        assert!(!worker::run_once(&server.state).await.unwrap());
    })
    .await;
}

#[tokio::test]
async fn nothing_is_queued_without_smtp() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        create_user(&server.pool, "quiet@example.test", false).await;
        let s = login(&server, "quiet@example.test", PASSWORD).await;
        change_password(&server, &s, PASSWORD, "another long passphrase").await;
        assert!(job_rows(&server).await.is_empty());
    })
    .await;
}
