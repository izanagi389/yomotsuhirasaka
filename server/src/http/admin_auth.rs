//! 管理API: ログイン・ログアウト・本人の情報・再認証・パスワードの変更・セッションの一覧。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | POST | /api/admin/session | ログイン（Cookieを設定し、CSRFトークンを本文で返す） |
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
use crate::auth::{password, session, throttle, users};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LoginResponse {
    csrf_token: String,
    user: Me,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Me {
    id: Uuid,
    email: String,
    is_server_admin: bool,
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

/// パスワードを照合し、アカウント単位の制限を記録する。待ち時間中は、正しいパスワードでも false。
/// 照合は待ち時間中や利用者がいないときも必ず行う（応答時間の差を小さくする。Q189）。
async fn check_password(
    state: &AppState,
    record: Option<users::LoginRecord>,
    presented: String,
) -> anyhow::Result<Option<Uuid>> {
    let stored = record
        .as_ref()
        .filter(|r| !r.disabled)
        .and_then(|r| r.password_hash.clone());
    let matched = password::verify(presented, stored).await?;
    let Some(record) = record else {
        return Ok(None);
    };
    if record.disabled {
        return Ok(None);
    }
    if record.blocked {
        // 待ち時間中の試行は数えない（待ち時間を延ばすのは、待ち時間の外での失敗だけ）
        return Ok(None);
    }
    if matched {
        throttle::record_success(&state.pool, record.id).await?;
        Ok(Some(record.id))
    } else {
        throttle::record_failure(&state.pool, record.id).await?;
        Ok(None)
    }
}

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
    let Some(user_id) = check_password(&state, record, req.password).await? else {
        return Err(ApiError::invalid_credentials());
    };

    // ログインの前に持っていたセッションは失効させ、必ず新しいセッションIDにする（Q107）
    if let Some(old) = cookie(&headers, SESSION_COOKIE) {
        session::revoke_presented(&state.pool, &old).await?;
    }
    let issued = session::issue(&state.pool, user_id, &state.config.session).await?;
    let current = session::resolve(&state.pool, &issued.token, &state.config.session)
        .await?
        .ok_or_else(ApiError::internal)?;
    let mut res = Json(LoginResponse {
        csrf_token: issued.csrf_token,
        user: Me {
            id: current.user_id,
            email: current.email,
            is_server_admin: current.is_server_admin,
        },
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
    Json(Me {
        id: current.user_id,
        email: current.email,
        is_server_admin: current.is_server_admin,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReauthRequest {
    password: Option<String>,
    totp_code: Option<String>,
}

fn reauth_failed() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "reauth_failed",
        "Identity confirmation failed.",
    )
}

/// 再認証。多要素認証を設定していない人はパスワードで行う。
/// 多要素認証（次の段階で追加）を設定した人は、多要素認証でだけ行えるようにする（Q108）。
pub async fn reauth(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    JsonBody(req): JsonBody<ReauthRequest>,
) -> Result<StatusCode, ApiError> {
    let presented = match (req.password, req.totp_code) {
        (Some(p), None) => p,
        // 多要素認証はまだ設定できないので、コードによる再認証は常に通らない
        (None, Some(_)) => return Err(reauth_failed()),
        _ => {
            return Err(ApiError::bad_request(
                "Send either `password` or `totpCode`.",
            ));
        }
    };
    let record = users::find_by_id_for_login(&state.pool, current.user_id).await?;
    if check_password(&state, record, presented).await?.is_none() {
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
