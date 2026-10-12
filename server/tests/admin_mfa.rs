//! 多要素認証（TOTPと復旧コード）の結合テスト（本物のPostgreSQLとHTTPを使う）。
//!
//! 環境変数 `CMS_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::server::*;
use common::with_scratch;

use totp_rs::Secret;

#[tokio::test]
async fn enroll_then_login_requires_second_factor() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let email = "mfa@example.test";
        create_user(&server.pool, email, false).await;
        let s1 = login(&server, email, PASSWORD).await;
        let s2 = login(&server, email, PASSWORD).await;

        // 設定の開始: 秘密とURL
        let setup = send(&server, "POST", "/api/admin/me/mfa/totp", &s1, None).await;
        assert_eq!(setup.status, 200, "{}", setup.body);
        let secret = setup.json_str("secret").unwrap();
        let url = setup.json_str("otpauthUrl").unwrap();
        assert!(
            url.starts_with("otpauth://totp/cms.example.test%3Amfa%40example.test?"),
            "{url}"
        );
        assert!(
            url.contains(&format!("secret={secret}")) && url.contains("issuer=cms.example.test")
        );

        // 秘密は暗号化して保存されている
        let raw = Secret::try_from_base32(&secret).unwrap();
        let stored: Vec<u8> =
            sqlx::query_scalar("SELECT totp_pending_secret_enc FROM cms.users WHERE email = $1")
                .bind(email)
                .fetch_one(&server.pool)
                .await
                .unwrap();
        assert!(
            !stored
                .windows(raw.as_bytes().len())
                .any(|w| w == raw.as_bytes())
        );

        // 誤ったコードでは確定しない
        let wrong = send(
            &server,
            "POST",
            "/api/admin/me/mfa/totp/confirm",
            &s1,
            Some("{\"code\":\"000000\"}"),
        )
        .await;
        assert_eq!(wrong.status, 422);
        // 正しいコードで確定し、復旧コードを10個受け取る
        let confirm_code = code(&secret, 0);
        let confirm = send(
            &server,
            "POST",
            "/api/admin/me/mfa/totp/confirm",
            &s1,
            Some(&format!("{{\"code\":\"{confirm_code}\"}}")),
        )
        .await;
        assert_eq!(confirm.status, 200, "{}", confirm.body);
        let codes = recovery_codes(&confirm.body);
        assert_eq!(codes.len(), 10);
        // 多要素認証の変更で、他のセッションは使えなくなる（Q107）
        assert_eq!(get(&server, "/api/admin/me", &s2).await.status, 401);
        let me = get(&server, "/api/admin/me", &s1).await;
        assert!(me.body.contains("\"mfaEnabled\":true"), "{}", me.body);
        // 二度目の設定はできない
        assert_eq!(
            send(&server, "POST", "/api/admin/me/mfa/totp", &s1, None)
                .await
                .status,
            409
        );

        // パスワードだけでは、ログインした状態にならない
        let (res, pending) = password_step(&server, email, PASSWORD).await;
        assert_eq!(res.status, 200);
        assert!(
            res.body.contains("\"mfaRequired\":true") && !res.body.contains("csrfToken"),
            "{}",
            res.body
        );
        let pending = pending.unwrap();
        let as_pending = Login {
            cookie: pending.clone(),
            csrf: String::new(),
        };
        assert_eq!(get(&server, "/api/admin/me", &as_pending).await.status, 401);
        assert_eq!(
            get(&server, "/api/admin/me/sessions", &as_pending)
                .await
                .status,
            401
        );

        // 確定に使ったコード（同じ区切り）は二度通らない
        let replay = mfa_step(
            &server,
            &pending,
            &format!("{{\"totpCode\":\"{confirm_code}\"}}"),
        )
        .await;
        assert_eq!(replay.status, 401);
        assert_eq!(
            replay.json_str("error").as_deref(),
            Some("invalid_mfa_code")
        );
        // 次の区切りのコードなら通る（前後1区切りまで許す）
        let ok = mfa_step(
            &server,
            &pending,
            &format!("{{\"totpCode\":\"{}\"}}", code(&secret, 1)),
        )
        .await;
        let full = session_from(&ok);
        assert_ne!(
            full.cookie, pending,
            "the session ID must change after the second factor"
        );
        assert_eq!(get(&server, "/api/admin/me", &full).await.status, 200);
        // 途中のセッションは使い終わった
        forget_used_steps(&server.pool, email).await;
        let again = mfa_step(
            &server,
            &pending,
            &format!("{{\"totpCode\":\"{}\"}}", code(&secret, 0)),
        )
        .await;
        assert_eq!(again.status, 401);
        assert_eq!(again.json_str("error").as_deref(), Some("unauthenticated"));

        // 復旧コードでもログインできる。同じ復旧コードは二度使えない
        let (_, pending) = password_step(&server, email, PASSWORD).await;
        let pending = pending.unwrap();
        let rc = &codes[0];
        let ok = mfa_step(
            &server,
            &pending,
            &format!("{{\"recoveryCode\":\"{}\"}}", rc.to_lowercase()),
        )
        .await;
        session_from(&ok);
        let (_, pending) = password_step(&server, email, PASSWORD).await;
        let reused = mfa_step(
            &server,
            &pending.unwrap(),
            &format!("{{\"recoveryCode\":\"{rc}\"}}"),
        )
        .await;
        assert_eq!(reused.status, 401);

        // 再認証: パスワードだけでは通らず、コードなら通る（Q108）
        let res = send(
            &server,
            "POST",
            "/api/admin/session/reauth",
            &full,
            Some(&format!("{{\"password\":\"{PASSWORD}\"}}")),
        )
        .await;
        assert_eq!(res.status, 403);
        forget_used_steps(&server.pool, email).await;
        let res = send(
            &server,
            "POST",
            "/api/admin/session/reauth",
            &full,
            Some(&format!("{{\"totpCode\":\"{}\"}}", code(&secret, 0))),
        )
        .await;
        assert_eq!(res.status, 204, "{}", res.body);
        let res = send(
            &server,
            "POST",
            "/api/admin/session/reauth",
            &full,
            Some(&format!("{{\"recoveryCode\":\"{}\"}}", codes[1])),
        )
        .await;
        assert_eq!(res.status, 204, "{}", res.body);
    })
    .await;
}

#[tokio::test]
async fn wrong_codes_count_toward_the_account_throttle() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let email = "brute@example.test";
        create_user(&server.pool, email, false).await;
        let s = login(&server, email, PASSWORD).await;
        let (secret, _) = enroll(&server, &s).await;

        let (_, pending) = password_step(&server, email, PASSWORD).await;
        let pending = pending.unwrap();
        for _ in 0..5 {
            assert_eq!(
                mfa_step(&server, &pending, "{\"totpCode\":\"000000\"}")
                    .await
                    .status,
                401
            );
        }
        // 待ち時間中は、正しいコードでも通らない
        forget_used_steps(&server.pool, email).await;
        let res = mfa_step(
            &server,
            &pending,
            &format!("{{\"totpCode\":\"{}\"}}", code(&secret, 0)),
        )
        .await;
        assert_eq!(res.status, 401);
        // 待ち時間中は、パスワードの段階も通らない（応答は誤ったパスワードと同じ）
        let (res, _) = password_step(&server, email, PASSWORD).await;
        assert_eq!(res.status, 401);
        assert_eq!(
            res.json_str("error").as_deref(),
            Some("invalid_credentials")
        );
    })
    .await;
}

#[tokio::test]
async fn regenerate_and_disable() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let email = "manage@example.test";
        create_user(&server.pool, email, false).await;
        let s = login(&server, email, PASSWORD).await;
        let (secret, body) = enroll(&server, &s).await;
        let old_codes = recovery_codes(&body);

        // 再認証の有効時間を過ぎると、作り直しも停止もできない
        expire_reauth(&server.pool, email).await;
        assert_eq!(send(&server, "POST", "/api/admin/me/mfa/recovery-codes", &s, None).await.status, 403);
        assert_eq!(send(&server, "DELETE", "/api/admin/me/mfa/totp", &s, None).await.status, 403);

        // 作り直すと、古い復旧コードは使えない
        forget_used_steps(&server.pool, email).await;
        let reauth = |c: String| format!("{{\"totpCode\":\"{c}\"}}");
        assert_eq!(send(&server, "POST", "/api/admin/session/reauth", &s, Some(&reauth(code(&secret, 0)))).await.status, 204);
        let res = send(&server, "POST", "/api/admin/me/mfa/recovery-codes", &s, None).await;
        assert_eq!(res.status, 200);
        let new_codes = recovery_codes(&res.body);
        assert_eq!(new_codes.len(), 10);
        let (_, pending) = password_step(&server, email, PASSWORD).await;
        let pending = pending.unwrap();
        assert_eq!(mfa_step(&server, &pending, &format!("{{\"recoveryCode\":\"{}\"}}", old_codes[0])).await.status, 401);
        let other = session_from(&mfa_step(&server, &pending, &format!("{{\"recoveryCode\":\"{}\"}}", new_codes[0])).await);

        // 止めると、他のセッションは使えなくなり、パスワードだけでログインできるようになる
        forget_used_steps(&server.pool, email).await;
        assert_eq!(send(&server, "POST", "/api/admin/session/reauth", &s, Some(&reauth(code(&secret, 0)))).await.status, 204);
        assert_eq!(send(&server, "DELETE", "/api/admin/me/mfa/totp", &s, None).await.status, 204);
        assert_eq!(get(&server, "/api/admin/me", &other).await.status, 401);
        let me = get(&server, "/api/admin/me", &s).await;
        assert!(me.body.contains("\"mfaEnabled\":false"), "{}", me.body);
        let remaining: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM cms.recovery_codes r JOIN cms.users u ON u.id = r.user_id WHERE u.email = $1",
        )
        .bind(email)
        .fetch_one(&server.pool)
        .await
        .unwrap();
        assert_eq!(remaining, 0);
        let (res, _) = password_step(&server, email, PASSWORD).await;
        assert!(res.body.contains("csrfToken"), "{}", res.body);
        // 停止の後は、コードでの再認証は通らない
        assert_eq!(send(&server, "POST", "/api/admin/session/reauth", &s, Some(&reauth(code(&secret, 0)))).await.status, 403);
    })
    .await;
}

#[tokio::test]
async fn mfa_required_for_everyone_limits_users_without_mfa() {
    with_scratch(|app, migration| async move {
        let mut cfg = config(&app, &migration);
        cfg.auth.mfa_required_for_everyone = true;
        let server = start(cfg).await;
        let email = "everyone@example.test";
        create_user(&server.pool, email, false).await;

        // ログインはでき、多要素認証の設定が必要だと分かる
        let res = login_raw(&server, email, PASSWORD, "198.18.30.1").await;
        assert!(
            res.body.contains("\"mfaSetupRequired\":true"),
            "{}",
            res.body
        );
        let s = login(&server, email, PASSWORD).await;
        let me = get(&server, "/api/admin/me", &s).await;
        assert_eq!(me.status, 200);
        assert!(me.body.contains("\"mfaSetupRequired\":true"), "{}", me.body);

        // 設定に必要な操作以外は使えない（INV-AUTH-25）
        for path in [
            "/api/admin/workspaces",
            "/api/admin/me/sessions",
            "/api/admin/users",
        ] {
            let res = get(&server, path, &s).await;
            assert_eq!(res.status, 403, "{path}");
            assert_eq!(
                res.json_str("error").as_deref(),
                Some("mfa_setup_required"),
                "{path}"
            );
        }
        let res = send(
            &server,
            "POST",
            "/api/admin/me/password",
            &s,
            Some(&format!(
                "{{\"currentPassword\":\"{PASSWORD}\",\"newPassword\":\"another long passphrase\"}}"
            )),
        )
        .await;
        assert_eq!(res.status, 403);

        // 再認証と、多要素認証の設定はできる
        let res = send(
            &server,
            "POST",
            "/api/admin/session/reauth",
            &s,
            Some(&format!("{{\"password\":\"{PASSWORD}\"}}")),
        )
        .await;
        assert_eq!(res.status, 204);
        enroll(&server, &s).await;

        // 設定した後は使える
        let me = get(&server, "/api/admin/me", &s).await;
        assert!(
            me.body.contains("\"mfaSetupRequired\":false"),
            "{}",
            me.body
        );
        assert_eq!(get(&server, "/api/admin/workspaces", &s).await.status, 200);
        assert_eq!(get(&server, "/api/admin/me/sessions", &s).await.status, 200);

        // ログアウトは、多要素認証がなくてもできる
        create_user(&server.pool, "leaver@example.test", false).await;
        let leaver = login(&server, "leaver@example.test", PASSWORD).await;
        assert_eq!(
            send(&server, "DELETE", "/api/admin/session", &leaver, None)
                .await
                .status,
            204
        );
    })
    .await;
}
