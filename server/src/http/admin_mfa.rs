//! 管理API: 本人の多要素認証（TOTPと復旧コード）の設定。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | POST | /api/admin/me/mfa/totp | TOTPの設定を始める（秘密と otpauth のURLを返す） |
//! | POST | /api/admin/me/mfa/totp/confirm | コードを確かめて有効にする（復旧コードを返す。他のセッションは失効） |
//! | DELETE | /api/admin/me/mfa/totp | TOTPを止める（復旧コードも消す。他のセッションは失効） |
//! | POST | /api/admin/me/mfa/recovery-codes | 復旧コードを作り直す |
//!
//! どれも再認証が必要（多要素認証を設定した人は、多要素認証で再認証する）。
//! 設定の開始と確定は、全員への多要素認証の必須（Q197）で他の操作を止められている人もできる。

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use super::AppState;
use super::admin_auth::notify;
use super::error::ApiError;
use super::extract::{AuthSession, AuthSessionForMfaSetup, JsonBody};
use crate::auth::{mfa, session};
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
        "Multi-factor authentication is already enabled. Disable it first to set it up again.",
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
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    if current.mfa_enabled {
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
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    if current.mfa_enabled {
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
    notify(&state, &state.pool, &current.email, Event::MfaEnabled).await?;
    // 今コードを確かめたので、このセッションは本人確認を済ませたものとする
    session::mark_reauthenticated(&state.pool, current.id).await?;
    Ok(Json(RecoveryCodes {
        recovery_codes: codes,
    }))
}

/// TOTPを止める。多要素認証の変更なので、他のセッションをすべて失効させる（Q107）。
pub async fn disable_totp(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
) -> Result<StatusCode, ApiError> {
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    if !current.mfa_enabled {
        return Err(not_enabled());
    }
    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    mfa::disable(&mut tx, current.user_id).await?;
    session::revoke_others(&mut *tx, current.user_id, current.id).await?;
    notify(&state, &mut *tx, &current.email, Event::MfaDisabled).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn regenerate_recovery_codes(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
) -> Result<Json<RecoveryCodes>, ApiError> {
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
