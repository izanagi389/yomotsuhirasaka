//! 管理APIのエラーの応答。本文は `{"error": "<code>", "message": "<English>"}`。
//!
//! 状態コード（Q185）: 認証の失敗は401、権限・CSRFは403、見つからないものは404、検証の失敗は4xx、回数制限は429。
//! 内部のエラーの詳細は応答に出さず、ログにだけ残す（ログにも秘密の値や本文は出さない）。

use std::borrow::Cow;

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: Cow<'static, str>,
    pub retry_after: Option<u64>,
}

#[derive(Serialize)]
struct Body<'a> {
    error: &'a str,
    message: &'a str,
}

impl ApiError {
    pub fn new(
        status: StatusCode,
        code: &'static str,
        message: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            retry_after: None,
        }
    }

    pub fn unauthenticated() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "Authentication is required.",
        )
    }

    /// ログインの失敗。理由（利用者がいない、パスワードが違う、待ち時間中、無効にされた）によらず同じ応答にする。
    pub fn invalid_credentials() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "The email or password is incorrect.",
        )
    }

    pub fn csrf() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "csrf_failed",
            "A valid CSRF token is required.",
        )
    }

    pub fn forbidden_origin() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "forbidden_origin",
            "Cross-origin requests are not allowed.",
        )
    }

    pub fn reauth_required() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "reauth_required",
            "Please confirm your identity again before this operation.",
        )
    }

    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "Not found.")
    }

    pub fn bad_request(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }

    pub fn rate_limited(retry_after: u64) -> Self {
        Self::too_many(
            "rate_limited",
            "Too many requests. Try again later.",
            retry_after,
        )
    }

    /// 回数制限で断る（429 と Retry-After）。何の上限かをエラーコードで分ける。
    pub fn too_many(code: &'static str, message: &'static str, retry_after: u64) -> Self {
        Self {
            retry_after: Some(retry_after),
            ..Self::new(StatusCode::TOO_MANY_REQUESTS, code, message)
        }
    }

    pub fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "An internal error occurred.",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut res = (
            self.status,
            Json(Body {
                error: self.code,
                message: &self.message,
            }),
        )
            .into_response();
        if let Some(secs) = self.retry_after {
            res.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(secs));
        }
        res
    }
}

impl From<crate::access::Denied> for ApiError {
    fn from(d: crate::access::Denied) -> Self {
        use crate::access::Denied;
        match d {
            Denied::NotFound => Self::not_found(),
            Denied::Forbidden => Self::new(
                StatusCode::FORBIDDEN,
                "forbidden",
                "You do not have permission for this operation.",
            ),
            Denied::MfaRequired => Self::new(
                StatusCode::FORBIDDEN,
                "mfa_required",
                "Set up multi-factor authentication before administrative operations.",
            ),
        }
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        // エラーの連鎖には、SQLの文や入力の値は含まれない（sqlx は束縛した値を表示しない）
        tracing::error!("request failed: {e:#}");
        Self::internal()
    }
}
