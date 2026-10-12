//! 管理APIのリクエストから取り出すもの: ログイン中のセッション、JSONの本文、接続元のIPアドレス。

use std::net::{IpAddr, SocketAddr};

use axum::Json;
use axum::extract::{ConnectInfo, FromRequest, FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, header};
use serde::de::DeserializeOwned;

use super::AppState;
use super::error::ApiError;
use crate::auth::{personal_tokens, session};

/// セッションのCookieの名前。`__Host-` の接頭辞により、Secure・Path=/・Domainなしでしか設定できない（Q91）。
pub const SESSION_COOKIE: &str = "__Host-cms_session";
/// CSRFトークンを送るヘッダー。
pub const CSRF_HEADER: &str = "x-csrf-token";

/// Cookieのヘッダーから、指定した名前の値を取り出す。
pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_owned())
        .filter(|v| !v.is_empty())
}

/// 状態を変えない（CSRFトークンの要らない）メソッドか。
fn is_safe(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// 状態を変えるリクエストに `Origin` が付いていれば、公開URLと同じオリジンであることを確かめる。
/// （ブラウザから別のサイト経由で送られたリクエストを止める。CSRFトークンと二重の守り）
pub fn check_origin(headers: &HeaderMap, public_origin: &str) -> Result<(), ApiError> {
    match headers.get(header::ORIGIN) {
        None => Ok(()),
        Some(v) if v.as_bytes() == public_origin.as_bytes() => Ok(()),
        Some(_) => Err(ApiError::forbidden_origin()),
    }
}

/// ログイン中のセッション（Cookieの経路）。状態を変えるメソッドでは、CSRFトークンも確かめる。
/// 全員への多要素認証の必須が有効なとき（Q197）、多要素認証のない利用者は通さない。
pub struct AuthSession(pub session::Current);

/// [`AuthSession`] と同じだが、多要素認証のない利用者も通す。
/// 多要素認証の設定に必要な操作（本人の情報の確認、多要素認証の設定、再認証、ログアウト）にだけ使う（INV-AUTH-25）。
pub struct AuthSessionForMfaSetup(pub session::Current);

async fn authenticate(parts: &mut Parts, state: &AppState) -> Result<session::Current, ApiError> {
    // Bearerトークンの経路ではCookieを読まない。Cookieの経路では必ずCSRFトークンを求める（Q110）
    if let Some(value) = parts.headers.get(header::AUTHORIZATION) {
        let presented = value
            .to_str()
            .ok()
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim)
            .ok_or_else(ApiError::unauthenticated)?;
        let current = personal_tokens::resolve(&state.pool, presented)
            .await?
            .ok_or_else(ApiError::unauthenticated)?;
        let grant = current
            .grant
            .as_ref()
            .expect("token sessions carry a grant");
        // 状態を変える操作には write の範囲が要る（実際にできるかは、さらに本人のロールで決まる）
        if !is_safe(&parts.method) && !grant.has_scope("write") {
            return Err(ApiError::new(
                axum::http::StatusCode::FORBIDDEN,
                "insufficient_scope",
                "This token is not allowed to make changes.",
            ));
        }
        return Ok(current);
    }
    let presented = cookie(&parts.headers, SESSION_COOKIE).ok_or_else(ApiError::unauthenticated)?;
    let current = session::resolve(&state.pool, &presented, &state.config.session)
        .await?
        .ok_or_else(ApiError::unauthenticated)?;
    if !is_safe(&parts.method) {
        check_origin(&parts.headers, &state.public_origin)?;
        let ok = parts
            .headers
            .get(CSRF_HEADER)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|t| current.csrf_matches(t));
        if !ok {
            return Err(ApiError::csrf());
        }
    }
    Ok(current)
}

/// Cookieのセッション（管理画面）でだけ許す操作か確かめる。
/// ログアウト・再認証・パスワードや多要素認証の変更・セッションやトークンの管理などは、個人トークンではできない。
pub fn require_cookie_session(current: &session::Current) -> Result<(), ApiError> {
    if current.via_token() {
        return Err(ApiError::new(
            axum::http::StatusCode::FORBIDDEN,
            "session_required",
            "This operation is only available from the admin screen, not with a personal token.",
        ));
    }
    Ok(())
}

/// 多要素認証の設定を済ませるまで、他の操作をさせないか。
pub fn mfa_setup_pending(state: &AppState, current: &session::Current) -> bool {
    state.config.auth.mfa_required_for_everyone && !current.mfa_enabled
}

impl FromRequestParts<AppState> for AuthSession {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let current = authenticate(parts, state).await?;
        if mfa_setup_pending(state, &current) {
            return Err(ApiError::new(
                axum::http::StatusCode::FORBIDDEN,
                "mfa_setup_required",
                "Set up multi-factor authentication before using the admin API.",
            ));
        }
        Ok(Self(current))
    }
}

impl FromRequestParts<AppState> for AuthSessionForMfaSetup {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        Ok(Self(authenticate(parts, state).await?))
    }
}

/// JSONの本文。読めないときは、入力の値を応答に含めない、決まった文のエラーにする。
pub struct JsonBody<T>(pub T);

impl<T, S> FromRequest<S> for JsonBody<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, ApiError> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(v)) => Ok(Self(v)),
            Err(_) => Err(ApiError::bad_request(
                "The request body must be JSON in the expected format (Content-Type: application/json).",
            )),
        }
    }
}

/// 接続元のIPアドレス。`X-Forwarded-For` は、信頼するプロキシから届いた場合だけ使う（Q106）。
pub struct ClientIp(pub IpAddr);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let ConnectInfo(peer) = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .copied()
            .ok_or_else(ApiError::internal)?;
        Ok(Self(client_ip(
            peer.ip(),
            &parts.headers,
            &state.config.trusted_proxies,
        )))
    }
}

/// 接続元が信頼するプロキシなら、`X-Forwarded-For` を右から読み、信頼するプロキシでない最初のアドレスを使う。
pub fn client_ip(peer: IpAddr, headers: &HeaderMap, trusted: &[IpAddr]) -> IpAddr {
    let peer = peer.to_canonical();
    if !trusted.contains(&peer) {
        return peer;
    }
    let hops: Vec<&str> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .collect();
    for hop in hops.iter().rev() {
        let Ok(ip) = hop.parse::<IpAddr>() else {
            // 読めない値があれば、それより左は信用しない
            return peer;
        };
        let ip = ip.to_canonical();
        if !trusted.contains(&ip) {
            return ip;
        }
    }
    peer
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn cookies() {
        let h = headers(&[
            ("cookie", "a=1; __Host-cms_session=abc; b=2"),
            ("cookie", "c=3"),
        ]);
        assert_eq!(cookie(&h, SESSION_COOKIE).as_deref(), Some("abc"));
        assert_eq!(cookie(&h, "c").as_deref(), Some("3"));
        assert_eq!(cookie(&h, "cms_session"), None);
        assert_eq!(
            cookie(
                &headers(&[("cookie", "__Host-cms_session=")]),
                SESSION_COOKIE
            ),
            None
        );
    }

    #[test]
    fn forwarded_for_is_used_only_from_trusted_proxies() {
        let proxy: IpAddr = "10.0.0.1".parse().unwrap();
        let h = headers(&[("x-forwarded-for", "203.0.113.9, 198.18.0.7")]);
        // 信頼していない接続元からの X-Forwarded-For は無視する
        assert_eq!(client_ip(proxy, &h, &[]), proxy);
        // 信頼するプロキシからなら、右端（プロキシが付けた値）を使う
        assert_eq!(
            client_ip(proxy, &h, &[proxy]),
            "198.18.0.7".parse::<IpAddr>().unwrap()
        );
        // 多段のプロキシ
        let inner: IpAddr = "198.18.0.7".parse().unwrap();
        assert_eq!(
            client_ip(proxy, &h, &[proxy, inner]),
            "203.0.113.9".parse::<IpAddr>().unwrap()
        );
        // 読めない値があれば、それより左は使わない
        let bad = headers(&[("x-forwarded-for", "198.18.0.1, garbage")]);
        assert_eq!(client_ip(proxy, &bad, &[proxy]), proxy);
        // ヘッダーがなければ接続元
        assert_eq!(client_ip(proxy, &HeaderMap::new(), &[proxy]), proxy);
    }

    #[test]
    fn origin() {
        let o = "https://cms.example.com";
        assert!(check_origin(&HeaderMap::new(), o).is_ok());
        assert!(check_origin(&headers(&[("origin", o)]), o).is_ok());
        assert!(check_origin(&headers(&[("origin", "https://evil.example")]), o).is_err());
        assert!(check_origin(&headers(&[("origin", "null")]), o).is_err());
    }
}
