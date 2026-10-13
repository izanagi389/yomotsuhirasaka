//! 管理API: 本人の多要素認証（TOTPと復旧コード）の設定。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | POST | /api/admin/me/mfa/totp | TOTPの設定を始める（秘密と otpauth のURLを返す） |
//! | POST | /api/admin/me/mfa/totp/confirm | コードを確かめて有効にする（復旧コードを返す。他のセッションは失効） |
//! | DELETE | /api/admin/me/mfa/totp | TOTPを止める（復旧コードも消す。他のセッションは失効） |
//! | POST | /api/admin/me/mfa/recovery-codes | 復旧コードを作り直す |
//! | GET | /api/admin/me/mfa/passkeys | 本人のパスキーの一覧 |
//! | POST | /api/admin/me/mfa/passkeys | パスキーの登録を始める（`navigator.credentials.create()` に渡す値を返す） |
//! | POST | /api/admin/me/mfa/passkeys/confirm | 登録を終える（最初の方法なら復旧コードを返す。他のセッションは失効） |
//! | DELETE | /api/admin/me/mfa/passkeys/{id} | パスキーを解除する（他のセッションは失効） |
//!
//! TOTPとパスキーは併用できる。どちらか1つでもあれば、多要素認証を設定していることになる。
//! 復旧コードは、最初の方法を設定したときに発行し、方法がすべてなくなったときに消す。
//!
//! どれも再認証が必要（多要素認証を設定した人は、多要素認証で再認証する）。
//! 設定の開始と確定は、全員への多要素認証の必須（Q197）で他の操作を止められている人もできる。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use super::AppState;
use super::admin_auth::notify;
use super::error::ApiError;
use super::extract::{AuthSession, AuthSessionForMfaSetup, JsonBody, require_cookie_session};
use crate::auth::{mfa, passkeys, session};
use crate::mail::notify::Event;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TotpSetup {
    /// 認証アプリに手で入力するための秘密（Base32）。この応答でだけ渡す。
    secret: String,
    /// 認証アプリに読み込ませるURL（QRコードは管理画面で作る）。
    otpauth_url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryCodes {
    /// 一度だけ表示する復旧コード。
    recovery_codes: Vec<String>,
}

fn already_enabled() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "mfa_already_enabled",
        "The authenticator app is already set up. Remove it first to set it up again.",
    )
}

fn not_enabled() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "mfa_not_enabled",
        "Multi-factor authentication is not enabled.",
    )
}

pub async fn begin_totp(
    State(state): State<AppState>,
    AuthSessionForMfaSetup(current): AuthSessionForMfaSetup,
) -> Result<Json<TotpSetup>, ApiError> {
    require_cookie_session(&current)?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    if mfa::totp_enabled(&state.pool, current.user_id).await? {
        return Err(already_enabled());
    }
    let enrollment = mfa::begin_enrollment(
        &state.pool,
        &state.keys,
        current.user_id,
        &current.email,
        &state.issuer,
    )
    .await?;
    Ok(Json(TotpSetup {
        secret: enrollment.secret_base32,
        otpauth_url: enrollment.otpauth_url,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfirmRequest {
    code: String,
}

/// 設定を確定する。多要素認証の変更なので、他のセッションをすべて失効させる（Q107）。
pub async fn confirm_totp(
    State(state): State<AppState>,
    AuthSessionForMfaSetup(current): AuthSessionForMfaSetup,
    JsonBody(req): JsonBody<ConfirmRequest>,
) -> Result<Json<RecoveryCodes>, ApiError> {
    require_cookie_session(&current)?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    if mfa::totp_enabled(&state.pool, current.user_id).await? {
        return Err(already_enabled());
    }
    let Some(codes) =
        mfa::confirm_enrollment(&state.pool, &state.keys, current.user_id, &req.code).await?
    else {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_mfa_code",
            "The verification code is incorrect, or setup was not started.",
        ));
    };
    session::revoke_others(&state.pool, current.user_id, current.id).await?;
    notify(&state, &state.pool, &current, Event::MfaEnabled).await?;
    // 今コードを確かめたので、このセッションは本人確認を済ませたものとする
    session::mark_reauthenticated(&state.pool, current.id).await?;
    Ok(Json(RecoveryCodes {
        recovery_codes: codes,
    }))
}

/// TOTPを止める（パスキーが残っていれば、多要素認証は続く）。多要素認証の変更なので、他のセッションをすべて失効させる（Q107）。
pub async fn disable_totp(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
) -> Result<StatusCode, ApiError> {
    require_cookie_session(&current)?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    if !mfa::totp_enabled(&state.pool, current.user_id).await? {
        return Err(not_enabled());
    }
    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    let still = mfa::remove_totp(&mut tx, current.user_id).await?;
    session::revoke_others(&mut *tx, current.user_id, current.id).await?;
    let event = if still {
        Event::TotpRemoved
    } else {
        Event::MfaDisabled
    };
    notify(&state, &mut *tx, &current, event).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn regenerate_recovery_codes(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
) -> Result<Json<RecoveryCodes>, ApiError> {
    require_cookie_session(&current)?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    if !current.mfa_enabled {
        return Err(not_enabled());
    }
    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    let codes = mfa::replace_recovery_codes(&mut tx, current.user_id).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(RecoveryCodes {
        recovery_codes: codes,
    }))
}

fn passkeys_unavailable() -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "passkeys_unavailable",
        "Passkeys are not available on this server.",
    )
}

/// パスキーの名前の上限（文字数）。
const MAX_PASSKEY_NAME_CHARS: usize = 100;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BeginPasskey {
    /// 見分けるための名前（例: 「仕事のノートPC」）。
    name: String,
}

pub async fn list_passkeys(
    State(state): State<AppState>,
    AuthSessionForMfaSetup(current): AuthSessionForMfaSetup,
) -> Result<Json<Vec<passkeys::Summary>>, ApiError> {
    require_cookie_session(&current)?;
    Ok(Json(passkeys::list(&state.pool, current.user_id).await?))
}

/// パスキーの登録を始める。再認証が必要。
pub async fn begin_passkey(
    State(state): State<AppState>,
    AuthSessionForMfaSetup(current): AuthSessionForMfaSetup,
    JsonBody(req): JsonBody<BeginPasskey>,
) -> Result<Json<webauthn_rs::prelude::CreationChallengeResponse>, ApiError> {
    require_cookie_session(&current)?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    let name = req.name.trim();
    if name.is_empty()
        || name.chars().count() > MAX_PASSKEY_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        return Err(ApiError::bad_request(
            "`name` must be 1 to 100 characters without control characters.",
        ));
    }
    let rp = state.passkeys.as_ref().ok_or_else(passkeys_unavailable)?;
    match passkeys::begin_registration(
        &state.pool,
        rp,
        current.id,
        current.user_id,
        &current.email,
        name,
    )
    .await?
    {
        Ok(challenge) => Ok(Json(challenge)),
        Err(_) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "too_many_passkeys",
            "Remove a passkey before adding another one.",
        )),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfirmPasskey {
    /// `navigator.credentials.create()` の結果。
    credential: Box<webauthn_rs::prelude::RegisterPublicKeyCredential>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PasskeyAdded {
    id: uuid::Uuid,
    /// 最初の多要素認証の方法だったときだけ、一度だけ表示する復旧コード。2つ目以降なら空。
    recovery_codes: Vec<String>,
}

/// パスキーの登録を終える。多要素認証の変更なので、他のセッションをすべて失効させる（Q107）。
pub async fn confirm_passkey(
    State(state): State<AppState>,
    AuthSessionForMfaSetup(current): AuthSessionForMfaSetup,
    JsonBody(req): JsonBody<ConfirmPasskey>,
) -> Result<(StatusCode, Json<PasskeyAdded>), ApiError> {
    require_cookie_session(&current)?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    let rp = state.passkeys.as_ref().ok_or_else(passkeys_unavailable)?;
    let registered = match passkeys::finish_registration(
        &state.pool,
        rp,
        current.id,
        current.user_id,
        &req.credential,
    )
    .await?
    {
        Ok(r) => r,
        Err(passkeys::Refused::AlreadyRegistered) => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "passkey_already_registered",
                "This passkey is already registered.",
            ));
        }
        Err(passkeys::Refused::TooMany) => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "too_many_passkeys",
                "Remove a passkey before adding another one.",
            ));
        }
        Err(_) => {
            return Err(ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_passkey",
                "The passkey could not be verified, or registration was not started.",
            ));
        }
    };
    session::revoke_others(&state.pool, current.user_id, current.id).await?;
    notify(&state, &state.pool, &current, Event::PasskeyAdded).await?;
    // 今パスキーで本人を確かめたので、このセッションは本人確認を済ませたものとする
    session::mark_reauthenticated(&state.pool, current.id).await?;
    Ok((
        StatusCode::CREATED,
        Json(PasskeyAdded {
            id: registered.id,
            recovery_codes: registered.recovery_codes,
        }),
    ))
}

/// パスキーを解除する。多要素認証の変更なので、他のセッションをすべて失効させる（Q107）。
pub async fn remove_passkey(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    require_cookie_session(&current)?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    let id = uuid::Uuid::parse_str(&id).map_err(|_| ApiError::not_found())?;
    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    if !passkeys::remove(&mut tx, current.user_id, id).await? {
        return Err(ApiError::not_found());
    }
    let still = mfa::forget_recovery_codes_if_no_method(&mut tx, current.user_id).await?;
    session::revoke_others(&mut *tx, current.user_id, current.id).await?;
    let event = if still {
        Event::PasskeyRemoved
    } else {
        Event::MfaDisabled
    };
    notify(&state, &mut *tx, &current, event).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}
