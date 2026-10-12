//! 管理APIのリクエストから取り出すもの: ログイン中のセッション、JSONの本文、接続元のIPアドレス。

use std::net::{IpAddr, SocketAddr};

use axum::Json;
use axum::extract::{ConnectInfo, FromRequest, FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, header};
use serde::de::DeserializeOwned;

use super::AppState;
use super::error::ApiError;
use crate::auth::session;

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
pub struct AuthSession(pub session::Current);

impl FromRequestParts<AppState> for AuthSession {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        // Bearerトークンの経路ではCookieを読まない（Q110）。個人トークンはまだ受け付けない。
        if parts.headers.contains_key(header::AUTHORIZATION) {
            return Err(ApiError::unauthenticated());
        }
        let presented =
            cookie(&parts.headers, SESSION_COOKIE).ok_or_else(ApiError::unauthenticated)?;
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
        Ok(Self(current))
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
