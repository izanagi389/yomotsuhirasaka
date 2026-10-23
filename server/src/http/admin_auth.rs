//! 管理API: ログイン・ログアウト・本人の情報・再認証・パスワードの変更・セッションの一覧。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | POST | /api/admin/session | ログイン（Cookieを設定し、CSRFトークンを本文で返す。多要素認証を設定した人には2段目を求める） |
//! | POST | /api/admin/session/mfa | ログインの2段目（TOTPか復旧コード） |
//! | DELETE | /api/admin/session | ログアウト |
//! | POST | /api/admin/session/reauth | 再認証 |
//! | GET | /api/admin/me | 本人の情報 |
//! | POST | /api/admin/me/password | パスワードの変更（再認証が必要。他のセッションは失効） |
//! | GET | /api/admin/me/sessions | 本人のログイン中のセッションの一覧 |
//! | DELETE | /api/admin/me/sessions/{id} | 本人のセッションを1つ失効させる |
//! | GET | /api/admin/me/login-history | 本人のログインの履歴（保存期間の内側だけ） |

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AppState;
use super::error::ApiError;
use super::extract::{
    AuthSession, AuthSessionForMfaSetup, ClientIp, JsonBody, SESSION_COOKIE, check_origin, cookie,
    mfa_setup_pending, require_cookie_session,
};
use crate::audit::{self, action};
use crate::auth::{devices, mfa, passkeys, password, session, throttle, users};
use crate::locale::Locale;
use crate::mail::notify::Event;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LoginRequest {
    email: String,
    password: String,
}

/// ログインの応答。多要素認証が必要なときは `mfaRequired: true` だけを返す。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LoginResponse {
    mfa_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    csrf_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<Me>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Me {
    id: Uuid,
    email: String,
    is_server_admin: bool,
    mfa_enabled: bool,
    /// 外部のOIDC/SSOでログインしたセッションか。
    sso: bool,
    /// 管理操作の前に、CMS側のパスキーの登録が必要か（SSOでログインした管理者で、パスキーがない。Q131）。
    passkey_required: bool,
    /// IdPでの再ログインで再認証できるか（SSOでログインした、管理者でない人。管理者はパスキーだけ）。
    idp_reauth_allowed: bool,
    /// 他の操作の前に、多要素認証の設定が必要か（全員への必須が有効で、まだ設定していない。Q197）。
    mfa_setup_required: bool,
    /// 本人が設定した言語（未設定なら null）。
    locale: Option<Locale>,
    /// 実際にメールに使う言語（未設定ならインスタンスの既定の言語）。
    effective_locale: Locale,
    /// 再認証が有効な残りの秒数（0なら、重い操作の前に再認証が要る）。
    reauth_seconds_left: i64,
    /// 本人が決めた表示名（未設定なら null。管理画面は、決めるまで入力を求める）。
    display_name: Option<String>,
    /// 表示名の入力欄の初期値（SSOのIdPが返した名前。本人にだけ返す）。
    display_name_suggestion: Option<String>,
}

impl Me {
    pub(super) fn new(state: &AppState, c: &session::Current) -> Self {
        Self {
            id: c.user_id,
            email: c.email.clone(),
            is_server_admin: c.is_server_admin,
            mfa_enabled: c.mfa_enabled,
            sso: c.sso,
            passkey_required: c.sso && c.admin_anywhere && !c.has_passkey,
            idp_reauth_allowed: c.sso && !c.admin_anywhere,
            mfa_setup_required: mfa_setup_pending(state, c),
            locale: c.locale,
            effective_locale: c.locale.unwrap_or(state.config.default_locale),
            reauth_seconds_left: c.reauth_seconds_left,
            display_name: None,
            display_name_suggestion: None,
        }
    }

    /// 表示名も読んで作る（応答に使うのはこちら）。
    pub(super) async fn load(state: &AppState, c: &session::Current) -> Result<Self, ApiError> {
        let mut me = Self::new(state, c);
        let names: Option<(Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT display_name, display_name_suggestion FROM cms.users WHERE id = $1",
        )
        .bind(c.user_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(anyhow::Error::from)?;
        if let Some((name, suggestion)) = names {
            me.display_name = name;
            me.display_name_suggestion = suggestion;
        }
        Ok(me)
    }
}

fn session_cookie(token: &str, max_age: u32) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={token}; Path=/; Max-Age={max_age}; Secure; HttpOnly; SameSite=Strict"
    ))
    .expect("token is URL-safe base64")
}

fn clear_session_cookie() -> HeaderValue {
    HeaderValue::from_static(
        "__Host-cms_session=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Strict",
    )
}

/// 本人への通知のメールを積む（SMTPの設定がなければ何もしない）。
pub(super) async fn notify<'e>(
    state: &AppState,
    executor: impl sqlx::PgExecutor<'e>,
    to: &session::Current,
    event: Event,
) -> anyhow::Result<()> {
    let locale = to.locale.unwrap_or(state.config.default_locale);
    let mail = crate::mail::notify::mail(&to.email, &state.issuer, event, locale);
    crate::mail::enqueue(executor, &state.keys, state.config.smtp.as_ref(), &mail).await?;
    Ok(())
}

/// アカウント単位の制限を守りながら、本人確認の結果を記録する。
/// 待ち時間中は結果によらず失敗とし、試行も数えない（待ち時間を延ばすのは、待ち時間の外での失敗だけ）。
async fn settle(
    state: &AppState,
    record: &users::LoginRecord,
    matched: bool,
) -> anyhow::Result<bool> {
    if record.disabled || record.blocked {
        return Ok(false);
    }
    if matched {
        throttle::record_success(&state.pool, record.id).await?;
    } else {
        throttle::record_failure(&state.pool, record.id).await?;
    }
    Ok(matched)
}

/// 新しいパスワードが漏えいの一覧に載っていれば断る（照会を有効にしたときだけ。Q170）。
/// 照会先に届かないときは通す（INV-AUTH-22）。
pub(super) async fn reject_breached(state: &AppState, new_password: &str) -> Result<(), ApiError> {
    let Some(config) = &state.config.auth.breached_passwords else {
        return Ok(());
    };
    match crate::auth::breached::check(state.egress.as_ref(), config, new_password).await {
        crate::auth::breached::Verdict::Breached => Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "breached_password",
            "This password appears in known data breaches. Choose a different one.",
        )),
        _ => Ok(()),
    }
}

/// パスワードを照合する。成功なら利用者の記録を返す。
/// 照合は待ち時間中や利用者がいないときも必ず行う（応答時間の差を小さくする。Q189）。
async fn check_password(
    state: &AppState,
    record: Option<users::LoginRecord>,
    presented: String,
) -> anyhow::Result<Option<users::LoginRecord>> {
    let stored = record
        .as_ref()
        .filter(|r| !r.disabled)
        .and_then(|r| r.password_hash.clone());
    let matched = password::verify(presented, stored).await?;
    let Some(record) = record else {
        return Ok(None);
    };
    Ok(settle(state, &record, matched).await?.then_some(record))
}

/// 多要素認証の2段目（TOTPのコード・復旧コード・パスキーの応答のどれか1つ）。
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecondFactor {
    totp_code: Option<String>,
    recovery_code: Option<String>,
    /// `navigator.credentials.get()` の結果。先に照合を始めておく（`.../passkey`）。
    passkey: Option<Box<webauthn_rs::prelude::PublicKeyCredential>>,
}

/// 多要素認証の2段目を確かめる。待ち時間中は確かめずに失敗とする（コードの総当たりを防ぐ）。
/// パスキーは、`challenge` のセッションと用途で始めた照合だけを受け付ける。
async fn check_second_factor(
    state: &AppState,
    record: &users::LoginRecord,
    factor: SecondFactor,
    challenge: (Uuid, passkeys::Purpose),
) -> anyhow::Result<bool> {
    if record.disabled || record.blocked || !record.mfa_enabled {
        return Ok(false);
    }
    let matched = match (factor.totp_code, factor.recovery_code, factor.passkey) {
        (Some(code), None, None) => {
            mfa::verify_totp(&state.pool, &state.keys, record.id, &code).await?
        }
        (None, Some(code), None) => mfa::use_recovery_code(&state.pool, record.id, &code).await?,
        (None, None, Some(response)) => match &state.passkeys {
            Some(rp) => {
                let (session_id, purpose) = challenge;
                passkeys::finish_authentication(
                    &state.pool,
                    rp,
                    session_id,
                    record.id,
                    purpose,
                    &response,
                )
                .await?
            }
            None => false,
        },
        _ => false,
    };
    settle(state, record, matched).await
}

fn device_cookie(value: &str) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{}={value}; Path=/; Max-Age={}; Secure; HttpOnly; SameSite=Strict",
        devices::DEVICE_COOKIE,
        devices::DEVICE_COOKIE_MAX_AGE,
    ))
    .expect("device id is URL-safe base64")
}

/// 発行したセッションと、端末のCookieの値。
pub(super) struct Established {
    pub issued: session::Issued,
    pub current: session::Current,
    pub device_token: String,
}

/// 本番のセッションを発行し、ログインを記録する（パスワードでもSSOでも共通）。
/// 端末と国の判定、ログインの履歴を記録し、新しい端末や国の変化なら本人に知らせる（13-security.md「本人への通知」）。
pub(super) async fn establish(
    state: &AppState,
    user_id: Uuid,
    ip: std::net::IpAddr,
    headers: &HeaderMap,
    sso: bool,
) -> Result<Established, ApiError> {
    let issued = if sso {
        session::issue_sso(&state.pool, user_id, &state.config.session).await?
    } else {
        session::issue(&state.pool, user_id, &state.config.session).await?
    };
    audit::record(&state.pool, None, Some(user_id), audit::action::LOGIN, &[]).await?;
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    let device = cookie(headers, devices::DEVICE_COOKIE);
    let country = state.geoip.as_ref().and_then(|g| g.country(ip));
    let recorded = devices::record_login(
        &state.pool,
        user_id,
        device.as_deref(),
        ip,
        user_agent,
        country.as_deref(),
    )
    .await?;
    let current = session::resolve(&state.pool, &issued.token, &state.config.session)
        .await?
        .ok_or_else(ApiError::internal)?;
    if let Some(notice) = recorded.notice {
        let event = match notice {
            devices::Notice::NewDevice => Event::NewDeviceLogin,
            devices::Notice::NewCountry => Event::NewCountryLogin,
        };
        let locale = current.locale.unwrap_or(state.config.default_locale);
        let mail = crate::mail::notify::login(
            &current.email,
            &state.issuer,
            locale,
            event,
            ip.to_canonical(),
            devices::browser_family(user_agent),
            country.as_deref(),
        );
        crate::mail::enqueue(&state.pool, &state.keys, state.config.smtp.as_ref(), &mail).await?;
    }
    Ok(Established {
        issued,
        current,
        device_token: recorded.device_token,
    })
}

/// セッションのCookieと端末のCookieを、応答に付ける。
pub(super) fn set_login_cookies(state: &AppState, res: &mut Response, established: &Established) {
    res.headers_mut().append(
        header::SET_COOKIE,
        session_cookie(
            &established.issued.token,
            state.config.session.absolute_lifetime_seconds,
        ),
    );
    res.headers_mut()
        .append(header::SET_COOKIE, device_cookie(&established.device_token));
}

/// パスワードでのログインを完了させ、本番のセッションを発行して応答を作る。
async fn complete_login(
    state: &AppState,
    user_id: Uuid,
    ip: std::net::IpAddr,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    let established = establish(state, user_id, ip, headers, false).await?;
    let mut res = Json(LoginResponse {
        mfa_required: false,
        csrf_token: Some(established.issued.csrf_token.clone()),
        user: Some(Me::load(state, &established.current).await?),
    })
    .into_response();
    set_login_cookies(state, &mut res, &established);
    Ok(res)
}

/// ログインの1段目（パスワード）。多要素認証を設定した人には、途中のセッションを発行して2段目を求める。
pub async fn login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    JsonBody(req): JsonBody<LoginRequest>,
) -> Result<Response, ApiError> {
    check_origin(&headers, &state.public_origin)?;
    state
        .login_limiter
        .check(ip)
        .map_err(ApiError::rate_limited)?;

    let record = match users::normalize_email(&req.email) {
        Some(email) => users::find_for_login(&state.pool, &email).await?,
        None => None,
    };
    let Some(record) = check_password(&state, record, req.password).await? else {
        return Err(ApiError::invalid_credentials());
    };

    // ログインの前に持っていたセッションは失効させ、必ず新しいセッションIDにする（Q107）
    if let Some(old) = cookie(&headers, SESSION_COOKIE) {
        session::revoke_presented(&state.pool, &old).await?;
    }
    if !record.mfa_enabled {
        return complete_login(&state, record.id, ip, &headers).await;
    }
    // 端末のCookieなどがあっても、多要素認証は省略しない（Q181）
    let pending = session::issue_mfa_pending(&state.pool, record.id).await?;
    let mut res = Json(LoginResponse {
        mfa_required: true,
        csrf_token: None,
        user: None,
    })
    .into_response();
    res.headers_mut().append(
        header::SET_COOKIE,
        session_cookie(&pending.token, session::MFA_PENDING_SECONDS),
    );
    Ok(res)
}

fn invalid_mfa_code() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "invalid_mfa_code",
        "The verification code is incorrect.",
    )
}

/// ログインの2段目（多要素認証のコード）。途中のセッションを失効させ、新しいセッションを発行する。
pub async fn login_mfa(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    JsonBody(factor): JsonBody<SecondFactor>,
) -> Result<Response, ApiError> {
    check_origin(&headers, &state.public_origin)?;
    state
        .login_limiter
        .check(ip)
        .map_err(ApiError::rate_limited)?;
    let presented = cookie(&headers, SESSION_COOKIE).ok_or_else(ApiError::unauthenticated)?;
    let (pending_id, user_id) = session::resolve_mfa_pending(&state.pool, &presented)
        .await?
        .ok_or_else(ApiError::unauthenticated)?;
    let record = users::find_by_id_for_login(&state.pool, user_id)
        .await?
        .ok_or_else(ApiError::unauthenticated)?;
    if !check_second_factor(
        &state,
        &record,
        factor,
        (pending_id, passkeys::Purpose::Login),
    )
    .await?
    {
        return Err(invalid_mfa_code());
    }
    session::revoke(&state.pool, pending_id).await?;
    complete_login(&state, user_id, ip, &headers).await
}

pub async fn logout(
    State(state): State<AppState>,
    AuthSessionForMfaSetup(current): AuthSessionForMfaSetup,
) -> Result<Response, ApiError> {
    require_cookie_session(&current)?;
    session::revoke(&state.pool, current.id).await?;
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut()
        .append(header::SET_COOKIE, clear_session_cookie());
    Ok(res)
}

pub async fn me(
    State(state): State<AppState>,
    AuthSessionForMfaSetup(current): AuthSessionForMfaSetup,
) -> Result<Json<impl Serialize>, ApiError> {
    Ok(Json(Me::load(&state, &current).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateMe {
    /// 言語。null で未設定に戻す（インスタンスの既定の言語を使う）。項目がなければ変えない。
    #[serde(default, deserialize_with = "present")]
    locale: Option<Option<String>>,
    /// 表示名（1〜50文字、制御文字なし。前後の空白は除く）。未設定には戻せない。
    display_name: Option<String>,
}

/// 項目が「ない」ことと「null」を区別する（ない: None、null: Some(None)）。
fn present<'de, D>(d: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(d).map(Some)
}

/// 本人の設定の変更（言語（Q198）と表示名）。表示名を変えられるのは本人だけで、変更は監査ログに残す（名前は書かない）。
pub async fn update_me(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    JsonBody(req): JsonBody<UpdateMe>,
) -> Result<Json<impl Serialize>, ApiError> {
    if req.locale.is_none() && req.display_name.is_none() {
        return Err(ApiError::bad_request(
            "Send `locale`, `displayName` or both.",
        ));
    }
    let mut updated = current;
    if let Some(locale) = req.locale {
        let locale: Option<Locale> = locale
            .map(|l| l.parse())
            .transpose()
            .map_err(|()| ApiError::bad_request("`locale` must be \"en\", \"ja\" or null."))?;
        sqlx::query("UPDATE cms.users SET locale = $2 WHERE id = $1")
            .bind(updated.user_id)
            .bind(locale.map(Locale::as_str))
            .execute(&state.pool)
            .await
            .map_err(anyhow::Error::from)?;
        updated.locale = locale;
    }
    if let Some(raw) = &req.display_name {
        let name = crate::auth::users::normalize_display_name(raw).ok_or_else(|| {
            ApiError::bad_request(
                "`displayName` must be 1 to 50 characters without control characters.",
            )
        })?;
        let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
        let changed = sqlx::query(
            "UPDATE cms.users SET display_name = $2, display_name_suggestion = NULL
             WHERE id = $1 AND display_name IS DISTINCT FROM $2",
        )
        .bind(updated.user_id)
        .bind(&name)
        .execute(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
        if changed.rows_affected() == 1 {
            audit::record(
                &mut *tx,
                None,
                Some(updated.user_id),
                action::DISPLAY_NAME_CHANGED,
                &[updated.user_id],
            )
            .await?;
        }
        tx.commit().await.map_err(anyhow::Error::from)?;
    }
    Ok(Json(Me::load(&state, &updated).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReauthRequest {
    password: Option<String>,
    totp_code: Option<String>,
    recovery_code: Option<String>,
    /// `navigator.credentials.get()` の結果。先に照合を始めておく（`/session/reauth/passkey`）。
    passkey: Option<Box<webauthn_rs::prelude::PublicKeyCredential>>,
}

fn reauth_failed() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "reauth_failed",
        "Identity confirmation failed.",
    )
}

/// 再認証。多要素認証を設定した人は多要素認証のコードでだけ、設定していない人はパスワードで行う（Q108）。
/// SSOでログインしたセッションでは、ここではパスキーだけを受け付ける（IdPでの再ログインは `/session/reauth/oidc`）。
pub async fn reauth(
    State(state): State<AppState>,
    AuthSessionForMfaSetup(current): AuthSessionForMfaSetup,
    JsonBody(req): JsonBody<ReauthRequest>,
) -> Result<StatusCode, ApiError> {
    require_cookie_session(&current)?;
    let given = [
        req.password.is_some(),
        req.totp_code.is_some(),
        req.recovery_code.is_some(),
        req.passkey.is_some(),
    ];
    if given.iter().filter(|g| **g).count() != 1 {
        return Err(ApiError::bad_request(
            "Send exactly one of `password`, `totpCode`, `recoveryCode` or `passkey`.",
        ));
    }
    // SSOでログインしたセッションは、IdPでの再ログインかパスキーで再認証する（Q108, Q131）
    if current.sso && req.passkey.is_none() {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "reauth_method_not_allowed",
            "This session signed in with SSO. Confirm your identity with a passkey or by signing in to the identity provider again.",
        ));
    }
    let method = if req.password.is_some() {
        session::Method::Password
    } else if req.totp_code.is_some() {
        session::Method::Totp
    } else if req.recovery_code.is_some() {
        session::Method::Recovery
    } else {
        session::Method::Passkey
    };
    let record = users::find_by_id_for_login(&state.pool, current.user_id)
        .await?
        .ok_or_else(ApiError::unauthenticated)?;
    let ok = match req.password {
        // 多要素認証を設定した人は、パスワードだけでは通さない
        Some(_) if record.mfa_enabled => false,
        Some(p) => check_password(&state, Some(record), p).await?.is_some(),
        None => {
            let factor = SecondFactor {
                totp_code: req.totp_code,
                recovery_code: req.recovery_code,
                passkey: req.passkey,
            };
            check_second_factor(
                &state,
                &record,
                factor,
                (current.id, passkeys::Purpose::Reauth),
            )
            .await?
        }
    };
    if !ok {
        return Err(reauth_failed());
    }
    session::mark_reauthenticated(&state.pool, current.id, method).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangePasswordRequest {
    current_password: String,
    new_password: String,
}

/// パスワードの変更。再認証が必要で、現在のパスワードも確かめる。変更したら、他のセッションをすべて失効させる（Q107）。
pub async fn change_password(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    JsonBody(req): JsonBody<ChangePasswordRequest>,
) -> Result<StatusCode, ApiError> {
    require_cookie_session(&current)?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    let words = password::personal_words(&current.email, &state.config.public_url);
    let refs: Vec<&str> = words.iter().map(String::as_str).collect();
    if let Err(weakness) = password::check_policy(&req.new_password, &refs) {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "weak_password",
            weakness.message(),
        ));
    }
    reject_breached(&state, &req.new_password).await?;
    let record = users::find_by_id_for_login(&state.pool, current.user_id).await?;
    if check_password(&state, record, req.current_password)
        .await?
        .is_none()
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "invalid_current_password",
            "The current password is incorrect.",
        ));
    }
    let hash = password::hash(req.new_password).await?;
    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    users::set_password(&mut *tx, current.user_id, &hash).await?;
    session::revoke_others(&mut *tx, current.user_id, current.id).await?;
    notify(&state, &mut *tx, &current, Event::PasswordChanged).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_sessions(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
) -> Result<Json<Vec<session::Summary>>, ApiError> {
    require_cookie_session(&current)?;
    Ok(Json(
        session::list_active(
            &state.pool,
            current.user_id,
            current.id,
            &state.config.session,
        )
        .await?,
    ))
}

pub async fn revoke_session(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    require_cookie_session(&current)?;
    let id = Uuid::parse_str(&id).map_err(|_| ApiError::not_found())?;
    if session::revoke_own(&state.pool, current.user_id, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found())
    }
}

/// 本人のログインの履歴（IPアドレス、ブラウザの種類、国、新しい端末だったか）。
pub async fn login_history(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
) -> Result<Json<Vec<devices::Entry>>, ApiError> {
    require_cookie_session(&current)?;
    Ok(Json(
        devices::list(
            &state.pool,
            current.user_id,
            state.config.login_history.retention_seconds,
        )
        .await?,
    ))
}

/// 本人の外部のアカウント（OIDC）のひも付け。IdPの利用者の識別子（sub）は返さない。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    id: Uuid,
    issuer: String,
    linked_at: String,
    last_login_at: Option<String>,
}

pub async fn list_identities(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
) -> Result<Json<Vec<Identity>>, ApiError> {
    require_cookie_session(&current)?;
    let rows: Vec<(Uuid, String, String, Option<String>)> = sqlx::query_as(
        r#"SELECT id, issuer,
                  to_char(linked_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'),
                  to_char(last_login_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"')
           FROM cms.external_identities WHERE user_id = $1 ORDER BY linked_at"#,
    )
    .bind(current.user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(anyhow::Error::from)?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, issuer, linked_at, last_login_at)| Identity {
                id,
                issuer,
                linked_at,
                last_login_at,
            })
            .collect(),
    ))
}

fn passkeys_unavailable() -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "passkeys_unavailable",
        "Passkeys are not available on this server.",
    )
}

fn no_passkeys() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "no_passkeys",
        "No passkey is registered for this account.",
    )
}

/// ログインの2段目をパスキーで行うための照合を始める（多要素認証の途中のセッションで呼ぶ）。
/// 返す値をブラウザの `navigator.credentials.get()` に渡し、その結果を `POST /session/mfa` の `passkey` で送る。
pub async fn begin_login_passkey(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    check_origin(&headers, &state.public_origin)?;
    state
        .login_limiter
        .check(ip)
        .map_err(ApiError::rate_limited)?;
    let rp = state.passkeys.as_ref().ok_or_else(passkeys_unavailable)?;
    let presented = cookie(&headers, SESSION_COOKIE).ok_or_else(ApiError::unauthenticated)?;
    let (pending_id, user_id) = session::resolve_mfa_pending(&state.pool, &presented)
        .await?
        .ok_or_else(ApiError::unauthenticated)?;
    match passkeys::begin_authentication(
        &state.pool,
        rp,
        pending_id,
        user_id,
        passkeys::Purpose::Login,
    )
    .await?
    {
        Ok(challenge) => Ok(Json(challenge).into_response()),
        Err(_) => Err(no_passkeys()),
    }
}

/// 再認証をパスキーで行うための照合を始める。結果を `POST /session/reauth` の `passkey` で送る。
pub async fn begin_reauth_passkey(
    State(state): State<AppState>,
    AuthSessionForMfaSetup(current): AuthSessionForMfaSetup,
) -> Result<Response, ApiError> {
    require_cookie_session(&current)?;
    let rp = state.passkeys.as_ref().ok_or_else(passkeys_unavailable)?;
    match passkeys::begin_authentication(
        &state.pool,
        rp,
        current.id,
        current.user_id,
        passkeys::Purpose::Reauth,
    )
    .await?
    {
        Ok(challenge) => Ok(Json(challenge).into_response()),
        Err(_) => Err(no_passkeys()),
    }
}
