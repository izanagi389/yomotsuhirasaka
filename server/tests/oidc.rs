//! OIDC/SSO の結合テスト（本物のPostgreSQLとHTTPを使う。IdPはこのテストの中の擬似IdP）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Form, State};
use axum::routing::post;
use axum::{Json, Router};
use base64::Engine as _;
use common::server::*;
use common::with_scratch;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};
use sha2::Digest as _;
use yomi_server::config::OidcConfig;

/// テスト用の固定の鍵（P-256）。このテストの擬似IdPだけが使う。
const IDP_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQg4sFD3cZ/1XsSs/b0
JtKR3tUOsE5sdE7+vcqKCO4ovf+hRANCAAR9yGz3hV2rqakV4aEL102V2zk1rbRZ
XQ/mLPQyWS6mDrkblbzvv1R3uo49srF41kCSPxEG/Dihjie3//DOB9yR
-----END PRIVATE KEY-----
";
fn idp_key() -> EncodingKey {
    let b64: String = IDP_KEY
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .unwrap();
    EncodingKey::from_ec_der(&der)
}
const CLIENT_ID: &str = "yomi-client";
const CLIENT_SECRET: &str = "idp-client-secret-0123456789abcdef";
const JIT_DOMAIN: &str = "jit.example.test";

#[derive(Clone)]
struct Idp {
    issuer: String,
    /// 認可コード → (IDトークンのクレーム, code_challenge)
    codes: Arc<Mutex<HashMap<String, (Value, String)>>>,
}

impl Idp {
    /// 認可のURLから、IdPが戻すコードを作る。`claims` に `nonce` がなければ、URLの値を入れる。
    fn authorize(&self, authorization_url: &str, mut claims: Value) -> (String, String) {
        let url = url::Url::parse(authorization_url).unwrap();
        let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], CLIENT_ID);
        assert_eq!(q["code_challenge_method"], "S256");
        assert!(q["scope"].split(' ').any(|s| s == "openid"));
        let obj = claims.as_object_mut().unwrap();
        obj.entry("iss").or_insert(json!(self.issuer));
        obj.entry("aud").or_insert(json!(CLIENT_ID));
        obj.entry("nonce").or_insert(json!(q["nonce"]));
        let now = chrono_now();
        obj.entry("iat").or_insert(json!(now));
        obj.entry("exp").or_insert(json!(now + 300));
        let code = format!("code-{}", uuid::Uuid::new_v4().simple());
        self.codes
            .lock()
            .unwrap()
            .insert(code.clone(), (claims, q["code_challenge"].clone()));
        (code, q["state"].clone())
    }
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

async fn token(
    State(idp): State<Idp>,
    Form(form): Form<HashMap<String, String>>,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    let Some((claims, challenge)) = idp.codes.lock().unwrap().remove(&form["code"]) else {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant"})),
        )
            .into_response();
    };
    // PKCE（S256）
    let verifier = form.get("code_verifier").cloned().unwrap_or_default();
    let computed = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sha2::Sha256::digest(verifier.as_bytes()));
    if computed != challenge {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant"})),
        )
            .into_response();
    }
    let key = idp_key();
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("k1".into());
    let id_token = jsonwebtoken::encode(&header, &claims, &key).unwrap();
    Json(json!({"access_token": "at", "token_type": "Bearer", "expires_in": 300, "id_token": id_token})).into_response()
}

async fn start_idp() -> Idp {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let idp = Idp {
        issuer: issuer.clone(),
        codes: Default::default(),
    };
    let key = idp_key();
    let mut jwk = jsonwebtoken::jwk::Jwk::from_encoding_key(&key, Algorithm::ES256).unwrap();
    jwk.common.key_id = Some("k1".into());
    let discovery = json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "jwks_uri": format!("{issuer}/jwks"),
        "response_types_supported": ["code"],
        "code_challenge_methods_supported": ["S256"],
    });
    let jwks = json!({ "keys": [jwk] });
    let app = Router::new()
        .route(
            "/.well-known/openid-configuration",
            axum::routing::get(move || async move { Json(discovery) }),
        )
        .route(
            "/jwks",
            axum::routing::get(move || async move { Json(jwks) }),
        )
        .route("/token", post(token))
        .with_state(idp.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    idp
}

fn oidc_config(
    app: &yomi_server::config::DatabaseConfig,
    migration: &yomi_server::config::DatabaseConfig,
    idp: &Idp,
) -> yomi_server::config::Config {
    let mut c = config(app, migration);
    c.auth.oidc = Some(OidcConfig {
        issuer: idp.issuer.clone(),
        client_id: CLIENT_ID.into(),
        client_secret: CLIENT_SECRET.into(),
        display_name: "Example SSO".into(),
        jit_domains: vec![JIT_DOMAIN.into()],
    });
    c.egress.internal_destination_exceptions = vec!["127.0.0.1".into()];
    c
}

fn cookie_value(res: &Resp, name: &str) -> Option<String> {
    res.set_cookies().into_iter().find_map(|c| {
        c.split(';')
            .next()?
            .strip_prefix(&format!("{name}="))
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    })
}

/// SSOを始め、IdPのクレームで戻る。戻り先の応答と、始めたときの短命のCookieを返す。
async fn run_flow(
    server: &Server,
    idp: &Idp,
    path: &str,
    body: &str,
    extra: &[(&str, &str)],
    claims: Value,
) -> Resp {
    let res = request(server, "POST", path, extra, Some(body)).await;
    assert_eq!(res.status, 200, "{}", res.body);
    let browser = cookie_value(&res, "__Host-yomi_oidc").expect("flow cookie");
    assert!(
        res.set_cookies()[0].contains("SameSite=Lax"),
        "{:?}",
        res.set_cookies()
    );
    let url = res.json_str("authorizationUrl").unwrap();
    let (code, state) = idp.authorize(&url, claims);
    callback(server, &browser, &state, &code).await
}

async fn callback(server: &Server, browser: &str, state: &str, code: &str) -> Resp {
    let path = format!("/api/admin/session/oidc/callback?code={code}&state={state}");
    let cookie = format!("__Host-yomi_oidc={browser}");
    request(server, "GET", &path, &[("Cookie", &cookie)], None).await
}

fn location(res: &Resp) -> &str {
    res.header("location").unwrap_or("")
}

/// 戻り先がセッションを発行したら、CSRFトークンを受け取ってログインの状態にする。
async fn complete(server: &Server, res: &Resp) -> Login {
    assert_eq!(res.status, 303, "{}", res.body);
    assert!(!location(res).contains("error="), "{}", location(res));
    let cookie = cookie_value(res, "__Host-yomi_session").expect("session cookie");
    let cookie_header = format!("__Host-yomi_session={cookie}");
    let done = request(
        server,
        "POST",
        "/api/admin/session/oidc/complete",
        &[("Cookie", &cookie_header)],
        None,
    )
    .await;
    assert_eq!(done.status, 200, "{}", done.body);
    // 一度だけ
    let again = request(
        server,
        "POST",
        "/api/admin/session/oidc/complete",
        &[("Cookie", &cookie_header)],
        None,
    )
    .await;
    assert_eq!(again.status, 409, "{}", again.body);
    Login {
        cookie,
        csrf: done.json_str("csrfToken").unwrap(),
    }
}

async fn user_exists(server: &Server, email: &str) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM yomi.users WHERE email = $1)")
        .bind(email)
        .fetch_one(&server.pool)
        .await
        .unwrap()
}

const LOGIN: &str = "/api/admin/session/oidc/start";

#[tokio::test]
async fn jit_requires_verified_email_and_matching_domain() {
    with_scratch(|app, migration| async move {
        let idp = start_idp().await;
        let server = start(oidc_config(&app, &migration, &idp)).await;

        let info = request(&server, "GET", "/api/admin/session/oidc", &[], None).await;
        assert_eq!(info.status, 200);
        assert!(
            info.body.contains("\"enabled\":true") && info.body.contains("Example SSO"),
            "{}",
            info.body
        );

        // email_verified がない・false・文字列の "true" なら作らない
        for verified in [Value::Null, json!(false), json!("true")] {
            let email = format!("u{}@{JIT_DOMAIN}", uuid::Uuid::new_v4().simple());
            let mut claims = json!({"sub": uuid::Uuid::new_v4().to_string(), "email": email});
            if !verified.is_null() {
                claims["email_verified"] = verified.clone();
            }
            let res = run_flow(&server, &idp, LOGIN, "{}", &[], claims).await;
            assert_eq!(res.status, 303);
            assert!(
                location(&res).ends_with("/admin/sso/complete?purpose=login&error=not_registered"),
                "{verified}: {}",
                location(&res)
            );
            assert!(cookie_value(&res, "__Host-yomi_session").is_none());
            assert!(!user_exists(&server, &email).await, "{verified}");
        }

        // ドメインが違えば作らない（サブドメインも）
        let email = format!("u@sub.{JIT_DOMAIN}");
        let res = run_flow(
            &server,
            &idp,
            LOGIN,
            "{}",
            &[],
            json!({"sub": "s-sub", "email": email, "email_verified": true}),
        )
        .await;
        assert!(location(&res).contains("error=not_registered"));
        assert!(!user_exists(&server, &email).await);

        // 確認済みで、ドメインが一致すれば作る。2回目は同じ利用者でログインする
        let email = format!("new@{JIT_DOMAIN}");
        let claims = json!({"sub": "s-new", "email": email, "email_verified": true});
        let s = complete(
            &server,
            &run_flow(&server, &idp, LOGIN, "{}", &[], claims.clone()).await,
        )
        .await;
        let me = get(&server, "/api/admin/me", &s).await;
        assert_eq!(me.status, 200, "{}", me.body);
        assert!(
            me.body.contains("\"sso\":true") && me.body.contains(&email),
            "{}",
            me.body
        );
        assert!(me.body.contains("\"idpReauthAllowed\":true"), "{}", me.body);
        let s2 = complete(
            &server,
            &run_flow(&server, &idp, LOGIN, "{}", &[], claims).await,
        )
        .await;
        assert_eq!(get(&server, "/api/admin/me", &s2).await.status, 200);
        let users: i64 = sqlx::query_scalar("SELECT count(*) FROM yomi.users WHERE email = $1")
            .bind(&email)
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert_eq!(users, 1);
        // SSOのセッションでは、パスワードでの再認証はできない
        let r = send(
            &server,
            "POST",
            "/api/admin/session/reauth",
            &s,
            Some(r#"{"password":"x"}"#),
        )
        .await;
        assert_eq!(r.status, 422, "{}", r.body);
    })
    .await;
}

#[tokio::test]
async fn existing_accounts_are_not_linked_by_email() {
    with_scratch(|app, migration| async move {
        let idp = start_idp().await;
        let server = start(oidc_config(&app, &migration, &idp)).await;
        let email = format!("existing@{JIT_DOMAIN}");
        create_user(&server.pool, &email, false).await;
        let res = run_flow(
            &server,
            &idp,
            LOGIN,
            "{}",
            &[],
            json!({"sub": "attacker", "email": email, "email_verified": true}),
        )
        .await;
        assert!(
            location(&res).contains("error=not_registered"),
            "{}",
            location(&res)
        );
        let linked: i64 = sqlx::query_scalar("SELECT count(*) FROM yomi.external_identities")
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert_eq!(linked, 0);
    })
    .await;
}

#[tokio::test]
async fn state_is_bound_to_the_browser_and_used_once() {
    with_scratch(|app, migration| async move {
        let idp = start_idp().await;
        let server = start(oidc_config(&app, &migration, &idp)).await;
        let claims =
            json!({"sub": "s1", "email": format!("a@{JIT_DOMAIN}"), "email_verified": true});
        let res = request(&server, "POST", LOGIN, &[], Some("{}")).await;
        let browser = cookie_value(&res, "__Host-yomi_oidc").unwrap();
        let (code, state) = idp.authorize(&res.json_str("authorizationUrl").unwrap(), claims);

        // 別のブラウザ（Cookieが違う・ない）では使えない
        let other = callback(&server, "someone-else", &state, &code).await;
        assert!(
            location(&other).ends_with("?error=invalid_state"),
            "{}",
            location(&other)
        );
        let none = request(
            &server,
            "GET",
            &format!("/api/admin/session/oidc/callback?code={code}&state={state}"),
            &[],
            None,
        )
        .await;
        assert!(location(&none).contains("error=invalid_state"));
        // 戻り先は、短命のCookieを消す
        assert!(
            none.set_cookies()
                .iter()
                .any(|c| c.starts_with("__Host-yomi_oidc=;") && c.contains("Max-Age=0"))
        );

        let ok = callback(&server, &browser, &state, &code).await;
        complete(&server, &ok).await;
        // 2回目は使えない
        let replay = callback(&server, &browser, &state, &code).await;
        assert!(
            location(&replay).contains("error=invalid_state"),
            "{}",
            location(&replay)
        );
    })
    .await;
}

#[tokio::test]
async fn id_tokens_are_verified() {
    with_scratch(|app, migration| async move {
        let idp = start_idp().await;
        let server = start(oidc_config(&app, &migration, &idp)).await;
        let base =
            || json!({"sub": "s-v", "email": format!("v@{JIT_DOMAIN}"), "email_verified": true});
        let mut cases = Vec::new();
        let mut c = base();
        c["nonce"] = json!("another-nonce");
        cases.push(("nonce", c));
        let mut c = base();
        c["aud"] = json!("someone-else");
        cases.push(("aud", c));
        let mut c = base();
        c["iss"] = json!("https://evil.example.test");
        cases.push(("iss", c));
        let mut c = base();
        c["exp"] = json!(chrono_now() - 3600);
        cases.push(("exp", c));
        let mut c = base();
        c["aud"] = json!([CLIENT_ID, "other"]);
        cases.push(("azp", c));
        for (what, claims) in cases {
            let res = run_flow(&server, &idp, LOGIN, "{}", &[], claims).await;
            assert!(
                location(&res).contains("error=invalid_id_token"),
                "{what}: {}",
                location(&res)
            );
            assert!(
                cookie_value(&res, "__Host-yomi_session").is_none(),
                "{what}"
            );
        }
        assert!(!user_exists(&server, &format!("v@{JIT_DOMAIN}")).await);
        // 複数の aud でも、azp がこのクライアントなら通す
        let mut c = base();
        c["aud"] = json!([CLIENT_ID, "other"]);
        c["azp"] = json!(CLIENT_ID);
        complete(&server, &run_flow(&server, &idp, LOGIN, "{}", &[], c).await).await;
    })
    .await;
}

#[tokio::test]
async fn sso_admins_need_a_passkey_for_admin_operations() {
    with_scratch(|app, migration| async move {
        let idp = start_idp().await;
        let server = start(oidc_config(&app, &migration, &idp)).await;
        let email = "admin@example.test";
        create_user(&server.pool, email, true).await;
        let pw = login(&server, email, PASSWORD).await;

        // 本人の操作でひも付ける（email_verified が必要）
        let link = "/api/admin/me/identities/oidc/start";
        let headers = [
            ("Cookie", pw.cookie_header()),
            ("X-CSRF-Token", pw.csrf.clone()),
        ];
        let h: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let res = run_flow(
            &server,
            &idp,
            link,
            "{}",
            &h,
            json!({"sub": "admin-sub", "email": email, "email_verified": false}),
        )
        .await;
        assert!(
            location(&res).ends_with("?purpose=link&error=email_not_verified"),
            "{}",
            location(&res)
        );
        let res = run_flow(
            &server,
            &idp,
            link,
            "{}",
            &h,
            json!({"sub": "admin-sub", "email": email, "email_verified": true}),
        )
        .await;
        assert!(
            location(&res).ends_with("?purpose=link"),
            "{}",
            location(&res)
        );
        assert!(cookie_value(&res, "__Host-yomi_session").is_none());
        // 同じ発行者の2つ目はひも付けない
        let res = run_flow(
            &server,
            &idp,
            link,
            "{}",
            &h,
            json!({"sub": "admin-sub-2", "email": email, "email_verified": true}),
        )
        .await;
        assert!(
            location(&res).contains("error=already_linked"),
            "{}",
            location(&res)
        );

        // SSOでログインした管理者: パスキーがないので管理操作はできない
        let sso = complete(
            &server,
            &run_flow(
                &server,
                &idp,
                LOGIN,
                "{}",
                &[],
                json!({"sub": "admin-sub", "email": email, "email_verified": true}),
            )
            .await,
        )
        .await;
        let me = get(&server, "/api/admin/me", &sso).await;
        assert!(me.body.contains("\"passkeyRequired\":true"), "{}", me.body);
        assert!(
            me.body.contains("\"idpReauthAllowed\":false"),
            "{}",
            me.body
        );
        let res = send(
            &server,
            "POST",
            "/api/admin/workspaces",
            &sso,
            Some(r#"{"name":"sso-admin-ws"}"#),
        )
        .await;
        assert_eq!(res.status, 403, "{}", res.body);
        assert!(res.body.contains("mfa_required"), "{}", res.body);
        // 管理者は、IdPでの再ログインでは再認証できない
        let res = send(
            &server,
            "POST",
            "/api/admin/session/reauth/oidc",
            &sso,
            None,
        )
        .await;
        assert_eq!(res.status, 422, "{}", res.body);
        // パスキーの登録は、ログインの直後なら始められる
        let res = send(
            &server,
            "POST",
            "/api/admin/me/mfa/passkeys",
            &sso,
            Some(r#"{"name":"key"}"#),
        )
        .await;
        assert_eq!(res.status, 200, "{}", res.body);

        let audit: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM yomi.audit_log WHERE action = 'auth.identity_linked'",
        )
        .fetch_one(&server.pool)
        .await
        .unwrap();
        assert_eq!(audit, 1);
    })
    .await;
}

#[tokio::test]
async fn invitations_can_be_accepted_with_sso() {
    with_scratch(|app, migration| async move {
        let idp = start_idp().await;
        let server = start(oidc_config(&app, &migration, &idp)).await;
        create_user(&server.pool, "root@example.test", true).await;
        let root = login(&server, "root@example.test", PASSWORD).await;
        enroll(&server, &root).await;
        let res = send(
            &server,
            "POST",
            "/api/admin/workspaces",
            &root,
            Some(r#"{"name":"Blog"}"#),
        )
        .await;
        assert_eq!(res.status, 201, "{}", res.body);
        let ws = res.json_str("id").unwrap();
        let invited = "invited@corp.example.test";
        let res = send(
            &server,
            "POST",
            &format!("/api/admin/workspaces/{ws}/invitations"),
            &root,
            Some(&format!(r#"{{"email":"{invited}","role":"editor"}}"#)),
        )
        .await;
        assert_eq!(res.status, 201, "{}", res.body);
        let at = res.body.find("#token=").expect("link") + "#token=".len();
        let token: String = res.body[at..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        let body = format!(r#"{{"invitationToken":"{token}"}}"#);
        // 宛先と違うメールアドレス、未確認のメールアドレスでは承諾しない
        let res = run_flow(
            &server,
            &idp,
            LOGIN,
            &body,
            &[],
            json!({"sub": "inv", "email": "other@corp.example.test", "email_verified": true}),
        )
        .await;
        assert!(
            location(&res).ends_with("?purpose=invite&error=email_mismatch"),
            "{}",
            location(&res)
        );
        let res = run_flow(
            &server,
            &idp,
            LOGIN,
            &body,
            &[],
            json!({"sub": "inv", "email": invited, "email_verified": false}),
        )
        .await;
        assert!(
            location(&res).contains("error=email_not_verified"),
            "{}",
            location(&res)
        );
        assert!(!user_exists(&server, invited).await);
        // JITの対象外のドメインでも、招待なら作る
        let s = complete(
            &server,
            &run_flow(
                &server,
                &idp,
                LOGIN,
                &body,
                &[],
                json!({"sub": "inv", "email": invited, "email_verified": true}),
            )
            .await,
        )
        .await;
        let res = get(&server, "/api/admin/workspaces", &s).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert!(res.body.contains(&ws), "{}", res.body);
        // 使った招待は、もう使えない
        let res = request(&server, "POST", LOGIN, &[], Some(&body)).await;
        assert_eq!(res.status, 400, "{}", res.body);
    })
    .await;
}
