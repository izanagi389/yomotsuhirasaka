//! メールの送信とジョブのキューの結合テスト（本物のPostgreSQLと、テストの中の擬似SMTPサーバー）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::server::*;
use common::smtp::*;
use common::with_scratch;
use yomi_server::worker;

async fn job_rows(server: &Server) -> Vec<(i32, Option<String>, bool, bool)> {
    sqlx::query_as(
        "SELECT attempts, last_error, failed_at IS NOT NULL, payload_enc IS NOT NULL FROM yomi.jobs ORDER BY created_at",
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
        let stored: Vec<u8> = sqlx::query_scalar("SELECT payload_enc FROM yomi.jobs")
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
        assert!(received[0].contains("From: CMS <yomi@example.test>"));
        // 本文は符号化されて届くので、読める形にしてから確かめる
        let body = decoded_body(&received[0]);
        // 言語を設定していなければ、インスタンスの既定の言語（英語）だけで書く（Q198）
        assert!(
            body.contains("password") && !body.contains("パスワード"),
            "{body}"
        );
        assert!(!body.contains(new_password) && !body.contains(PASSWORD));
        assert!(job_rows(&server).await.is_empty());

        // 一時的な失敗はやり直す（すぐにはやり直さない）
        smtp.lock().unwrap().transient_failures = 1;
        // 言語を日本語にすると、以後の通知は日本語だけで届く
        let res = send(
            &server,
            "PATCH",
            "/api/admin/me",
            &s,
            Some("{\"locale\":\"ja\"}"),
        )
        .await;
        assert_eq!(res.status, 200, "{}", res.body);
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
        sqlx::query("UPDATE yomi.jobs SET run_at = now()")
            .execute(&server.pool)
            .await
            .unwrap();
        assert!(worker::run_once(&server.state).await.unwrap());
        assert!(job_rows(&server).await.is_empty());
        assert_eq!(smtp.lock().unwrap().received.len(), 2);
        let ja = decoded_body(&smtp.lock().unwrap().received[1]);
        assert!(
            ja.contains("多要素認証") && !ja.contains("authentication"),
            "{ja}"
        );
        assert!(
            !decoded_body(&smtp.lock().unwrap().received[1]).contains(&secret),
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
