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

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AppState;
use super::error::ApiError;
use super::extract::{AuthSession, ClientIp, JsonBody, SESSION_COOKIE, check_origin, cookie};
use crate::auth::{mfa, password, session, throttle, users};

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
struct Me {
    id: Uuid,
    email: String,
    is_server_admin: bool,
    mfa_enabled: bool,
}

impl From<&session::Current> for Me {
    fn from(c: &session::Current) -> Self {
        Self {
            id: c.user_id,
            email: c.email.clone(),
            is_server_admin: c.is_server_admin,
            mfa_enabled: c.mfa_enabled,
        }
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

/// 多要素認証のコード（TOTPか復旧コードのどちらか一方）。
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecondFactor {
    totp_code: Option<String>,
    recovery_code: Option<String>,
}

/// 多要素認証のコードを確かめる。待ち時間中は確かめずに失敗とする（コードの総当たりを防ぐ）。
async fn check_second_factor(
    state: &AppState,
    record: &users::LoginRecord,
    factor: SecondFactor,
) -> anyhow::Result<bool> {
    if record.disabled || record.blocked || !record.mfa_enabled {
        return Ok(false);
    }
    let matched = match (factor.totp_code, factor.recovery_code) {
        (Some(code), None) => mfa::verify_totp(&state.pool, &state.keys, record.id, &code).await?,
        (None, Some(code)) => mfa::use_recovery_code(&state.pool, record.id, &code).await?,
        _ => false,
    };
    settle(state, record, matched).await
}

/// ログインを完了させ、本番のセッションを発行して応答を作る。
async fn complete_login(state: &AppState, user_id: Uuid) -> Result<Response, ApiError> {
    let issued = session::issue(&state.pool, user_id, &state.config.session).await?;
    let current = session::resolve(&state.pool, &issued.token, &state.config.session)
        .await?
        .ok_or_else(ApiError::internal)?;
    let mut res = Json(LoginResponse {
        mfa_required: false,
        csrf_token: Some(issued.csrf_token),
        user: Some(Me::from(&current)),
    })
    .into_response();
    res.headers_mut().append(
        header::SET_COOKIE,
        session_cookie(
            &issued.token,
            state.config.session.absolute_lifetime_seconds,
        ),
    );
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
        return complete_login(&state, record.id).await;
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
    if !check_second_factor(&state, &record, factor).await? {
        return Err(invalid_mfa_code());
    }
    session::revoke(&state.pool, pending_id).await?;
    complete_login(&state, user_id).await
}

pub async fn logout(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
) -> Result<Response, ApiError> {
    session::revoke(&state.pool, current.id).await?;
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut()
        .append(header::SET_COOKIE, clear_session_cookie());
    Ok(res)
}

pub async fn me(AuthSession(current): AuthSession) -> Json<impl Serialize> {
    Json(Me::from(&current))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReauthRequest {
    password: Option<String>,
    totp_code: Option<String>,
    recovery_code: Option<String>,
}

fn reauth_failed() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "reauth_failed",
        "Identity confirmation failed.",
    )
}

/// 再認証。多要素認証を設定した人は多要素認証のコードでだけ、設定していない人はパスワードで行う（Q108）。
pub async fn reauth(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    JsonBody(req): JsonBody<ReauthRequest>,
) -> Result<StatusCode, ApiError> {
    let given = [
        req.password.is_some(),
        req.totp_code.is_some(),
        req.recovery_code.is_some(),
    ];
    if given.iter().filter(|g| **g).count() != 1 {
        return Err(ApiError::bad_request(
            "Send exactly one of `password`, `totpCode` or `recoveryCode`.",
        ));
    }
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
            };
            check_second_factor(&state, &record, factor).await?
        }
    };
    if !ok {
        return Err(reauth_failed());
    }
    session::mark_reauthenticated(&state.pool, current.id).await?;
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
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    if let Err(weakness) = password::check_policy(&req.new_password) {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "weak_password",
            weakness.message(),
        ));
    }
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
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_sessions(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
) -> Result<Json<Vec<session::Summary>>, ApiError> {
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
    let id = Uuid::parse_str(&id).map_err(|_| ApiError::not_found())?;
    if session::revoke_own(&state.pool, current.user_id, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found())
    }
}
