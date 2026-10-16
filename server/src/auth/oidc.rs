//! 外部のOIDC/SSOのクライアント（13-security.md「OIDC/SSO」、Q103, Q126）。
//!
//! - 認可コードフロー。`state`・`nonce`・PKCE（S256）を必ず使う。値の生成と認可のURL・トークンの要求は `oauth2`、
//!   IDトークンの署名と `iss`・`aud`・`exp` の検証は `jsonwebtoken` に任せる（暗号処理は自作しない）。
//! - IdPとの通信（設定の取得・JWKS・トークンの要求）は、すべて外部への送信の共通の仕組み（egress）を通す。
//! - `nonce` は、始めたときに保存した値と照らし合わせる。複数の `aud` があるときは `azp` がこのクライアントであることも確かめる。
//! - `email_verified` は真偽値の `true` だけを確認済みとする（文字列や欠落は確認できないものとして扱う。INV-AUTH-19）。
//! - 再認証では `prompt=login` と `max_age=0` を付け、`auth_time` が始めた後であることを確かめる（Q108）。

use std::time::{Duration, Instant};

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use oauth2::basic::{
    BasicErrorResponse, BasicRevocationErrorResponse, BasicTokenIntrospectionResponse,
    BasicTokenType,
};
use oauth2::{
    AuthUrl, AuthorizationCode, Client, ClientId, ClientSecret, CsrfToken, EndpointNotSet,
    EndpointSet, ExtraTokenFields, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
    StandardRevocableToken, StandardTokenResponse, TokenUrl,
};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::config::OidcConfig;
use crate::egress::{Egress, EgressError, EgressRequest};

/// IdPの設定と鍵を持っておく時間。鍵が見つからなければ、この時間内でも取り直す。
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
/// IdPの応答の本文の上限。
const MAX_BODY: usize = 256 * 1024;
/// 時計のずれの許容（秒）。
const LEEWAY: u64 = 60;
/// IDトークンの署名に受け付けるアルゴリズム（非対称鍵だけ。`none` と HMAC は受け付けない）。
const ALGORITHMS: [Algorithm; 6] = [
    Algorithm::RS256,
    Algorithm::RS384,
    Algorithm::RS512,
    Algorithm::PS256,
    Algorithm::ES256,
    Algorithm::ES384,
];

/// 失敗の種類（応答やログには種類だけを出す。トークンや秘密の値は出さない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OidcError {
    /// IdPに届かない、またはIdPの応答が読めない。
    Unreachable(&'static str),
    /// IdPの設定がおかしい（発行者が違う、S256のPKCEがない、など）。
    BadDiscovery,
    /// トークンの要求が断られた（コードが無効など）。
    TokenRejected,
    /// IDトークンの検証に失敗した。
    InvalidIdToken,
}

impl std::fmt::Display for OidcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(code) => write!(f, "identity provider unreachable ({code})"),
            Self::BadDiscovery => f.write_str("identity provider configuration is not usable"),
            Self::TokenRejected => f.write_str("identity provider rejected the token request"),
            Self::InvalidIdToken => f.write_str("ID token is not valid"),
        }
    }
}

impl std::error::Error for OidcError {}

#[derive(Debug, Clone, Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    #[serde(default)]
    code_challenge_methods_supported: Vec<String>,
}

/// トークンの応答の、IDトークン。
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct IdTokenField {
    id_token: String,
}

impl ExtraTokenFields for IdTokenField {}

type TokenResponse = StandardTokenResponse<IdTokenField, BasicTokenType>;
type OidcClient = Client<
    BasicErrorResponse,
    TokenResponse,
    BasicTokenIntrospectionResponse,
    StandardRevocableToken,
    BasicRevocationErrorResponse,
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointSet,
>;

/// 認可を始めたときの値。`state` は照合のため、`nonce` と `pkce_verifier` は戻ってきたときに使う。
pub struct Started {
    pub authorization_url: String,
    pub state: String,
    pub nonce: String,
    pub pkce_verifier: String,
}

/// 検証を通ったIDトークンのクレーム。
#[derive(Debug, Clone)]
pub struct Verified {
    pub issuer: String,
    pub subject: String,
    /// 小文字にそろえたメールアドレス（なければ None）。
    pub email: Option<String>,
    /// `email_verified` が真偽値の true のときだけ true。
    pub email_verified: bool,
}

#[derive(Debug, Deserialize)]
struct Claims {
    sub: String,
    iss: String,
    #[serde(default)]
    aud: Audience,
    #[serde(default)]
    azp: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: Option<serde_json::Value>,
    #[serde(default)]
    auth_time: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(untagged)]
enum Audience {
    #[default]
    None,
    // 値そのものの検証は jsonwebtoken が行う。ここでは数だけを使う
    One(#[allow(dead_code)] String),
    Many(Vec<String>),
}

impl Audience {
    fn len(&self) -> usize {
        match self {
            Self::None => 0,
            Self::One(_) => 1,
            Self::Many(v) => v.len(),
        }
    }
}

/// HTTPの失敗（oauth2 のクライアントに返す）。
#[derive(Debug)]
struct HttpError(EgressError);

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.code())
    }
}

impl std::error::Error for HttpError {}

pub struct Oidc {
    config: OidcConfig,
    redirect_uri: String,
    discovery: Mutex<Option<(Instant, Discovery)>>,
    jwks: Mutex<Option<(Instant, JwkSet)>>,
}

/// 発行者の比較（末尾の `/` の有無は区別しない）。
fn same_issuer(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

async fn get_json<T: serde::de::DeserializeOwned>(
    egress: &Egress,
    url: &str,
) -> Result<T, OidcError> {
    let res = egress
        .send(EgressRequest {
            method: hyper::Method::GET,
            url: url.to_owned(),
            headers: vec![(
                hyper::header::ACCEPT,
                hyper::header::HeaderValue::from_static("application/json"),
            )],
            body: Vec::new(),
            read_body_limit: Some(MAX_BODY),
        })
        .await
        .map_err(|e| OidcError::Unreachable(e.code()))?;
    if !(200..300).contains(&res.status) {
        return Err(OidcError::Unreachable("http_status"));
    }
    serde_json::from_slice(&res.body.unwrap_or_default()).map_err(|_| OidcError::BadDiscovery)
}

impl Oidc {
    /// `public_url` は CMS の公開URL（戻り先は `<public_url>/api/admin/session/oidc/callback`）。
    pub fn new(config: OidcConfig, public_url: &str) -> Self {
        Self {
            config,
            redirect_uri: format!(
                "{}/api/admin/session/oidc/callback",
                public_url.trim_end_matches('/')
            ),
            discovery: Mutex::new(None),
            jwks: Mutex::new(None),
        }
    }

    pub fn issuer(&self) -> &str {
        self.config.issuer.trim_end_matches('/')
    }

    pub fn display_name(&self) -> &str {
        &self.config.display_name
    }

    /// JITの対象のドメインか（メールアドレスの `@` より後ろと完全に一致）。
    pub fn jit_allows(&self, email: &str) -> bool {
        email
            .rsplit_once('@')
            .is_some_and(|(_, domain)| self.config.jit_domains.iter().any(|d| d == domain))
    }

    async fn discovery(&self, egress: &Egress) -> Result<Discovery, OidcError> {
        let mut cached = self.discovery.lock().await;
        if let Some((at, d)) = cached.as_ref()
            && at.elapsed() < CACHE_TTL
        {
            return Ok(d.clone());
        }
        let url = format!("{}/.well-known/openid-configuration", self.issuer());
        let d: Discovery = get_json(egress, &url).await?;
        // 発行者が設定と違う設定は使わない。S256 の PKCE に対応していない IdP も使わない
        if !same_issuer(&d.issuer, self.issuer())
            || !d
                .code_challenge_methods_supported
                .iter()
                .any(|m| m == "S256")
        {
            return Err(OidcError::BadDiscovery);
        }
        *cached = Some((Instant::now(), d.clone()));
        Ok(d)
    }

    fn client(&self, d: &Discovery) -> Result<OidcClient, OidcError> {
        Ok(Client::new(ClientId::new(self.config.client_id.clone()))
            .set_client_secret(ClientSecret::new(self.config.client_secret.clone()))
            .set_auth_uri(
                AuthUrl::new(d.authorization_endpoint.clone())
                    .map_err(|_| OidcError::BadDiscovery)?,
            )
            .set_token_uri(
                TokenUrl::new(d.token_endpoint.clone()).map_err(|_| OidcError::BadDiscovery)?,
            )
            .set_redirect_uri(
                RedirectUrl::new(self.redirect_uri.clone()).map_err(|_| OidcError::BadDiscovery)?,
            ))
    }

    /// 認可を始める。`reauth` なら、IdPでもう一度ログインさせる（`prompt=login`、`max_age=0`）。
    pub async fn begin(&self, egress: &Egress, reauth: bool) -> Result<Started, OidcError> {
        let d = self.discovery(egress).await?;
        let client = self.client(&d)?;
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let nonce = CsrfToken::new_random();
        let mut req = client
            .authorize_url(CsrfToken::new_random)
            .add_scope(Scope::new("openid".into()))
            .add_scope(Scope::new("email".into()))
            .add_extra_param("nonce", nonce.secret())
            .set_pkce_challenge(challenge);
        if reauth {
            req = req
                .add_extra_param("prompt", "login")
                .add_extra_param("max_age", "0");
        }
        let (url, state) = req.url();
        Ok(Started {
            authorization_url: url.to_string(),
            state: state.secret().clone(),
            nonce: nonce.secret().clone(),
            pkce_verifier: verifier.secret().clone(),
        })
    }

    /// 認可コードをIDトークンに換え、検証する。`authenticated_since` があれば、`auth_time` がそれ以後であることも求める。
    pub async fn finish(
        &self,
        egress: &Egress,
        code: &str,
        pkce_verifier: String,
        nonce: &str,
        authenticated_since: Option<i64>,
    ) -> Result<Verified, OidcError> {
        let d = self.discovery(egress).await?;
        let client = self.client(&d)?;
        // 送信の共通の仕組みを通す（借用を持たない future にして、ハンドラーの future が Send になるようにする）
        let http = |req: oauth2::HttpRequest| {
            let egress = egress.clone();
            async move {
                let (parts, body) = req.into_parts();
                let res = egress
                    .send(EgressRequest {
                        method: parts.method,
                        url: parts.uri.to_string(),
                        headers: parts
                            .headers
                            .into_iter()
                            .filter_map(|(k, v)| k.map(|k| (k, v)))
                            .collect(),
                        body,
                        read_body_limit: Some(MAX_BODY),
                    })
                    .await
                    .map_err(HttpError)?;
                oauth2::http::Response::builder()
                    .status(res.status)
                    .header(oauth2::http::header::CONTENT_TYPE, "application/json")
                    .body(res.body.unwrap_or_default())
                    .map_err(|_| HttpError(EgressError::HttpFailed))
            }
        };
        let token = client
            .exchange_code(AuthorizationCode::new(code.to_owned()))
            .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier))
            .request_async(&http)
            .await
            .map_err(|_| OidcError::TokenRejected)?;
        let id_token = token.extra_fields().id_token.as_str();
        let claims = self.verify(egress, &d, id_token).await?;
        // nonce: 始めたときの値と同じであること（比較の時間は内容によらない）
        let nonce_ok = claims
            .nonce
            .as_deref()
            .is_some_and(|n| crate::auth::token::matches(n, &crate::auth::token::digest(nonce)));
        if !nonce_ok {
            return Err(OidcError::InvalidIdToken);
        }
        // 複数の aud があるときは、azp がこのクライアントであること
        if claims.aud.len() > 1 && claims.azp.as_deref() != Some(self.config.client_id.as_str()) {
            return Err(OidcError::InvalidIdToken);
        }
        if let Some(since) = authenticated_since {
            let leeway = i64::try_from(LEEWAY).unwrap_or(60);
            if !claims.auth_time.is_some_and(|t| t + leeway >= since) {
                return Err(OidcError::InvalidIdToken);
            }
        }
        Ok(Verified {
            issuer: self.issuer().to_owned(),
            subject: claims.sub,
            email: claims
                .email
                .map(|e| e.trim().to_lowercase())
                .filter(|e| e.contains('@')),
            email_verified: claims.email_verified == Some(serde_json::Value::Bool(true)),
        })
    }

    async fn jwks(
        &self,
        egress: &Egress,
        d: &Discovery,
        refresh: bool,
    ) -> Result<JwkSet, OidcError> {
        let mut cached = self.jwks.lock().await;
        if !refresh
            && let Some((at, set)) = cached.as_ref()
            && at.elapsed() < CACHE_TTL
        {
            return Ok(set.clone());
        }
        let set: JwkSet = get_json(egress, &d.jwks_uri).await?;
        *cached = Some((Instant::now(), set.clone()));
        Ok(set)
    }

    async fn verify(
        &self,
        egress: &Egress,
        d: &Discovery,
        id_token: &str,
    ) -> Result<Claims, OidcError> {
        let header =
            jsonwebtoken::decode_header(id_token).map_err(|_| OidcError::InvalidIdToken)?;
        if !ALGORITHMS.contains(&header.alg) {
            return Err(OidcError::InvalidIdToken);
        }
        let kid = header.kid.ok_or(OidcError::InvalidIdToken)?;
        // 知らない鍵なら、鍵の入れ替えかもしれないので一度だけ取り直す
        let mut set = self.jwks(egress, d, false).await?;
        if set.find(&kid).is_none() {
            set = self.jwks(egress, d, true).await?;
        }
        let jwk = set.find(&kid).ok_or(OidcError::InvalidIdToken)?;
        let key = DecodingKey::from_jwk(jwk).map_err(|_| OidcError::InvalidIdToken)?;
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[d.issuer.as_str()]);
        validation.set_audience(&[self.config.client_id.as_str()]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.leeway = LEEWAY;
        let data = jsonwebtoken::decode::<Claims>(id_token, &key, &validation)
            .map_err(|_| OidcError::InvalidIdToken)?;
        if !same_issuer(&data.claims.iss, self.issuer()) {
            return Err(OidcError::InvalidIdToken);
        }
        Ok(data.claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oidc(jit: &[&str]) -> Oidc {
        Oidc::new(
            OidcConfig {
                issuer: "https://idp.example.test/".into(),
                client_id: "cms".into(),
                client_secret: "secret-secret-secret-secret".into(),
                display_name: "SSO".into(),
                jit_domains: jit.iter().map(|s| (*s).to_owned()).collect(),
            },
            "https://cms.example.test",
        )
    }

    #[test]
    fn jit_domain_must_match_exactly() {
        let o = oidc(&["example.com"]);
        assert!(o.jit_allows("a@example.com"));
        assert!(!o.jit_allows("a@sub.example.com"));
        assert!(!o.jit_allows("a@example.com.evil.test"));
        assert!(!o.jit_allows("example.com"));
        assert_eq!(o.issuer(), "https://idp.example.test");
        assert_eq!(
            o.redirect_uri,
            "https://cms.example.test/api/admin/session/oidc/callback"
        );
    }
}
