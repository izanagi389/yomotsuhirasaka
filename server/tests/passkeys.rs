//! パスキーの結合テスト（本物のPostgreSQLとHTTPを使う）。
//!
//! 環境変数 `CMS_TEST_PG_SUPERUSER_URL` があるときだけ実行する。
//! ブラウザと認証器の代わりに、passkey-client と passkey-authenticator（ソフトウェアの認証器）を使う。

mod common;

use common::server::*;
use common::with_scratch;
use passkey_authenticator::{Authenticator, MemoryStore, MockUserValidationMethod, UserCheck};
use passkey_client::{Client, DefaultClientData};
use passkey_types::ctap2::Aaguid;
use passkey_types::webauthn::{CredentialCreationOptions, CredentialRequestOptions};
use webauthn_rs::prelude::Url;

const ORIGIN: &str = "https://cms.example.test";

/// 利用者の確認（生体認証など）を必ず通す、ソフトウェアの認証器を持ったクライアント。
macro_rules! device {
    () => {{
        let mut user = MockUserValidationMethod::new();
        user.expect_is_verification_enabled()
            .returning(|| Some(true));
        user.expect_is_presence_enabled().returning(|| true);
        user.expect_check_user()
            .returning(|_, presence, verification| {
                Ok(UserCheck {
                    presence,
                    verification,
                })
            });
        Client::new(Authenticator::new(
            Aaguid::new_empty(),
            MemoryStore::new(),
            user,
            passkey_crypto::AvailableBackend,
        ))
    }};
}

/// サーバーの登録の開始の応答から、資格情報を作って JSON にする。
macro_rules! create {
    ($client:expr, $options:expr) => {{
        let options: CredentialCreationOptions =
            serde_json::from_str(&$options).expect("creation options");
        let origin = Url::parse(ORIGIN).unwrap();
        let created = $client
            .register(&origin, options, DefaultClientData)
            .await
            .expect("register");
        serde_json::to_string(&created).unwrap()
    }};
}

/// サーバーの照合の開始の応答から、署名を作って JSON にする。
macro_rules! assert_with {
    ($client:expr, $options:expr) => {{
        let options: CredentialRequestOptions =
            serde_json::from_str(&$options).expect("request options");
        let origin = Url::parse(ORIGIN).unwrap();
        let asserted = $client
            .authenticate(&origin, options, DefaultClientData)
            .await
            .expect("authenticate");
        serde_json::to_string(&asserted).unwrap()
    }};
}

fn count_codes(body: &str) -> usize {
    recovery_codes(body).len()
}

#[tokio::test]
async fn passkeys_as_a_second_factor() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let email = "pk@example.test";
        create_user(&server.pool, email, false).await;
        let s = login(&server, email, PASSWORD).await;
        let mut laptop = device!();

        // 名前の検査
        for bad in ["{\"name\":\"\"}", "{\"name\":\"a\\nb\"}", "{}"] {
            let res = send(&server, "POST", "/api/admin/me/mfa/passkeys", &s, Some(bad)).await;
            assert!(res.status == 400 || res.status == 422, "{bad}: {}", res.body);
        }

        // 登録: 最初の方法なので復旧コードが出る
        let res = send(&server, "POST", "/api/admin/me/mfa/passkeys", &s, Some("{\"name\":\"laptop\"}")).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let credential = create!(laptop, res.body);
        let res = send(&server, "POST", "/api/admin/me/mfa/passkeys/confirm", &s, Some(&format!("{{\"credential\":{credential}}}"))).await;
        assert_eq!(res.status, 201, "{}", res.body);
        assert!(count_codes(&res.body) >= 8, "{}", res.body);
        // 同じ応答は二度使えない
        let res = send(&server, "POST", "/api/admin/me/mfa/passkeys/confirm", &s, Some(&format!("{{\"credential\":{credential}}}"))).await;
        assert_eq!(res.status, 422, "{}", res.body);
        let me = get(&server, "/api/admin/me", &s).await;
        assert!(me.body.contains("\"mfaEnabled\":true"), "{}", me.body);
        let list = get(&server, "/api/admin/me/mfa/passkeys", &s).await;
        assert!(list.body.contains("\"name\":\"laptop\"") && list.body.contains("\"lastUsedAt\":null"), "{}", list.body);

        // ログイン: パスワードの後、パスキーで2段目
        let (res, pending) = password_step(&server, email, PASSWORD).await;
        assert!(res.body.contains("\"mfaRequired\":true"));
        let pending = pending.unwrap();
        let begin = |pending: String| {
            let server = &server;
            async move {
                request(
                    server,
                    "POST",
                    "/api/admin/session/mfa/passkey",
                    &[("Cookie", &format!("__Host-cms_session={pending}")), ("X-Forwarded-For", "198.18.60.1")],
                    None,
                )
                .await
            }
        };
        let challenge = begin(pending.clone()).await;
        assert_eq!(challenge.status, 200, "{}", challenge.body);
        let assertion = assert_with!(laptop, challenge.body);
        let ok = mfa_step(&server, &pending, &format!("{{\"passkey\":{assertion}}}")).await;
        let s2 = session_from(&ok);
        assert_eq!(get(&server, "/api/admin/me", &s2).await.status, 200);
        let list = get(&server, "/api/admin/me/mfa/passkeys", &s2).await;
        assert!(!list.body.contains("\"lastUsedAt\":null"), "{}", list.body);

        // 同じ署名を使い回せない（チャレンジは一度だけ）
        let (_, pending) = password_step(&server, email, PASSWORD).await;
        let res = mfa_step(&server, &pending.clone().unwrap(), &format!("{{\"passkey\":{assertion}}}")).await;
        assert_eq!(res.status, 401, "{}", res.body);

        // 別の認証器では通らない
        let other = "other@example.test";
        create_user(&server.pool, other, false).await;
        let o = login(&server, other, PASSWORD).await;
        let mut phone = device!();
        let res = send(&server, "POST", "/api/admin/me/mfa/passkeys", &o, Some("{\"name\":\"phone\"}")).await;
        let credential = create!(phone, res.body);
        assert_eq!(send(&server, "POST", "/api/admin/me/mfa/passkeys/confirm", &o, Some(&format!("{{\"credential\":{credential}}}"))).await.status, 201);
        let pending = pending.unwrap();
        let challenge = begin(pending.clone()).await;
        // phone には pk@ の資格情報がないので、署名を作れない。phone の資格情報で作った署名を送っても通らない
        let (_, other_pending) = password_step(&server, other, PASSWORD).await;
        let other_challenge = begin(other_pending.unwrap()).await;
        let wrong = assert_with!(phone, other_challenge.body);
        let res = mfa_step(&server, &pending, &format!("{{\"passkey\":{wrong}}}")).await;
        assert_eq!(res.status, 401, "{}", res.body);
        let _ = challenge;

        // 再認証をパスキーで
        expire_reauth(&server.pool, email).await;
        let res = send(&server, "POST", "/api/admin/me/mfa/passkeys", &s2, Some("{\"name\":\"second\"}")).await;
        assert_eq!(res.status, 403, "{}", res.body);
        let res = send(&server, "POST", "/api/admin/session/reauth/passkey", &s2, None).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let assertion = assert_with!(laptop, res.body);
        let res = send(&server, "POST", "/api/admin/session/reauth", &s2, Some(&format!("{{\"passkey\":{assertion}}}"))).await;
        assert_eq!(res.status, 204, "{}", res.body);
        // 多要素認証を設定した人は、パスワードだけでは再認証できない
        let res = send(&server, "POST", "/api/admin/session/reauth", &s2, Some(&format!("{{\"password\":\"{PASSWORD}\"}}"))).await;
        assert_eq!(res.status, 403);

        // TOTPを2つ目の方法として足す: 復旧コードは作り直さない
        let res = send(&server, "POST", "/api/admin/me/mfa/totp", &s2, None).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let secret = res.json_str("secret").unwrap();
        let res = send(&server, "POST", "/api/admin/me/mfa/totp/confirm", &s2, Some(&format!("{{\"code\":\"{}\"}}", code(&secret, 0)))).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert!(res.body.contains("\"recoveryCodes\":[]"), "{}", res.body);
        let codes: i64 = sqlx::query_scalar("SELECT count(*) FROM cms.recovery_codes rc JOIN cms.users u ON u.id = rc.user_id WHERE u.email = $1")
            .bind(email)
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert!(codes >= 8);

        // TOTPを止めても、パスキーで多要素認証は続く（復旧コードも残る）
        assert_eq!(send(&server, "DELETE", "/api/admin/me/mfa/totp", &s2, None).await.status, 204);
        assert!(get(&server, "/api/admin/me", &s2).await.body.contains("\"mfaEnabled\":true"));

        // パスキーを外すと、多要素認証はなくなり、復旧コードも消える
        let id = get(&server, "/api/admin/me/mfa/passkeys", &s2).await.json_str("id").unwrap();
        assert_eq!(send(&server, "DELETE", &format!("/api/admin/me/mfa/passkeys/{id}"), &o, None).await.status, 404);
        assert_eq!(send(&server, "DELETE", &format!("/api/admin/me/mfa/passkeys/{id}"), &s2, None).await.status, 204);
        assert!(get(&server, "/api/admin/me", &s2).await.body.contains("\"mfaEnabled\":false"));
        let codes: i64 = sqlx::query_scalar("SELECT count(*) FROM cms.recovery_codes rc JOIN cms.users u ON u.id = rc.user_id WHERE u.email = $1")
            .bind(email)
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert_eq!(codes, 0);
        // ログインは、パスワードだけで済むようになる
        let res = login_raw(&server, email, PASSWORD, "198.18.60.2").await;
        assert!(res.body.contains("\"mfaRequired\":false"), "{}", res.body);

        // 登録の途中の状態は、始めたセッションでしか使えない
        let a = login(&server, email, PASSWORD).await;
        let b = login(&server, email, PASSWORD).await;
        let res = send(&server, "POST", "/api/admin/me/mfa/passkeys", &a, Some("{\"name\":\"x\"}")).await;
        let credential = create!(laptop, res.body);
        let res = send(&server, "POST", "/api/admin/me/mfa/passkeys/confirm", &b, Some(&format!("{{\"credential\":{credential}}}"))).await;
        assert_eq!(res.status, 422, "{}", res.body);

        // 多要素認証のリセット（CLIと同じ処理）は、パスキーも消す
        let mut tx = server.pool.begin().await.unwrap();
        let other_id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM cms.users WHERE email = $1").bind(other).fetch_one(&mut *tx).await.unwrap();
        cms_server::auth::mfa::disable(&mut tx, other_id).await.unwrap();
        tx.commit().await.unwrap();
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM cms.passkeys WHERE user_id = $1").bind(other_id).fetch_one(&server.pool).await.unwrap();
        assert_eq!(left, 0);
    })
    .await;
}
