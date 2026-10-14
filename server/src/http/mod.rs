//! HTTPの受け付け: ルーティング、共通の応答ヘッダー、アプリの状態。

pub mod admin_api_keys;
pub mod admin_auth;
pub mod admin_entries;
pub mod admin_invitations;
pub mod admin_media;
pub mod admin_mfa;
pub mod admin_publishing;
pub mod admin_resets;
pub mod admin_templates;
pub mod admin_tokens;
pub mod admin_translations;
pub mod admin_webhooks;
pub mod admin_workspaces;
pub mod delivery;
pub mod error;
pub mod extract;

use std::ops::Deref;
use std::sync::Arc;

use axum::Router;
use axum::http::{HeaderName, HeaderValue, header};
use axum::response::Response;
use axum::routing::{delete, get, post, put};
use sqlx::PgPool;

use crate::auth::throttle::IpLimiter;
use crate::config::Config;
use crate::db::Ident;
use crate::keys::KeyRing;
use error::ApiError;

/// リクエストの処理で共有する状態。
#[derive(Clone)]
pub struct AppState(Arc<Inner>);

pub struct Inner {
    /// 普段用の接続のプール。
    pub pool: PgPool,
    /// 移行・ロールの作成用の接続（1本）。ワークスペースのスキーマとロールの作成にだけ使う（Q173, Q179）。
    pub migration: PgPool,
    /// 普段用の接続のロール（ワークスペースのロールへの切り替えの権限を付ける相手）。
    pub app_role: Ident,
    pub config: Config,
    /// 公開URLのオリジン（`https://host[:port]`）。`Origin` ヘッダーの確認に使う。
    pub public_origin: String,
    /// 認証アプリに表示する発行者の名前（公開URLのホスト名）。
    pub issuer: String,
    pub login_limiter: IpLimiter,
    /// 暗号化の鍵（TOTPの秘密などの保存に使う）。
    pub keys: KeyRing,
    /// 国を調べるデータ（設定したときだけ。Q175）。
    pub geoip: Option<crate::geoip::GeoIp>,
    /// 配信APIのスキーマ（テンプレートの版ごと）。
    pub graphql: crate::delivery::schema::Cache,
    /// 配信APIの回数制限。
    pub delivery_limiter: crate::delivery::limiter::Limiter,
    /// パスキーの照合の設定（公開URLから作る。作れなければ起動時の検査で止める）。
    pub passkeys: Option<webauthn_rs::prelude::Webauthn>,
    /// 外部へのHTTPの送信の共通の仕組み（作れなければ起動時の検査で止める。作れないまま動いた場合は送らない）。
    pub egress: Option<crate::egress::Egress>,
}

impl Deref for AppState {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl AppState {
    pub fn new(
        pool: PgPool,
        migration: PgPool,
        app_role: Ident,
        config: Config,
        keys: KeyRing,
    ) -> Self {
        let public_origin = origin_of(&config.public_url);
        let issuer = public_origin
            .split_once("://")
            .map_or(public_origin.as_str(), |(_, host)| host)
            .to_owned();
        let login_limiter = IpLimiter::per_minute(config.limits.login_attempts_per_ip_per_minute);
        // 起動時の設定の検査で、読み込めることは確かめてある。ここで失敗したら国を調べずに続ける
        let geoip = config
            .login_history
            .geoip_database
            .as_deref()
            .and_then(|path| {
                crate::geoip::GeoIp::open(path)
                    .inspect_err(|e| tracing::error!("GeoIP database is not available: {e:#}"))
                    .ok()
            });
        let passkeys = crate::auth::passkeys::relying_party(&config.public_url, &issuer)
            .inspect_err(|e| tracing::error!("passkeys are not available: {e:#}"))
            .ok();
        let egress = crate::egress::Egress::from_config(&config.egress)
            .inspect_err(|e| tracing::error!("outbound HTTP is not available: {e:#}"))
            .ok();
        Self(Arc::new(Inner {
            egress,
            pool,
            migration,
            app_role,
            config,
            public_origin,
            issuer,
            login_limiter,
            keys,
            geoip,
            passkeys,
            graphql: Default::default(),
            delivery_limiter: Default::default(),
        }))
    }
}

/// URLのオリジン（スキーム・ホスト・ポート）。末尾のスラッシュやパスは除く。
fn origin_of(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("https", url));
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    format!(
        "{}://{}",
        scheme.to_ascii_lowercase(),
        authority.to_ascii_lowercase()
    )
}

/// すべての応答に付けるセキュリティヘッダー（Q114）。
/// 管理APIはHTMLを返さないので、CSPはすべてを禁じる。管理画面（SPA）は配信のときに自分のCSPを付ける。
const SECURITY_HEADERS: &[(&str, &str)] = &[
    (
        "content-security-policy",
        "default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
    ),
    (
        "strict-transport-security",
        "max-age=63072000; includeSubDomains",
    ),
    ("x-content-type-options", "nosniff"),
    ("x-frame-options", "DENY"),
    ("referrer-policy", "no-referrer"),
    ("cross-origin-opener-policy", "same-origin"),
    ("cross-origin-resource-policy", "same-origin"),
    ("cache-control", "no-store"),
];

async fn security_headers(mut res: Response) -> Response {
    let headers = res.headers_mut();
    for (name, value) in SECURITY_HEADERS {
        let name = HeaderName::from_static(name);
        if !headers.contains_key(&name) {
            headers.insert(name, HeaderValue::from_static(value));
        }
    }
    // CORSは許さない（Q110）。どこかで付けられても消す
    headers.remove(header::ACCESS_CONTROL_ALLOW_ORIGIN);
    headers.remove(header::ACCESS_CONTROL_ALLOW_CREDENTIALS);
    res
}

/// URLのクエリに配信APIのキーや個人トークンらしい値があれば、処理せずに拒否する（INV-AUTH-11）。
/// ヘッダーにも正しく付いていても拒否する（URLに載った時点で、ログなどに漏れたものとして扱う）。
async fn reject_credentials_in_url(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if req
        .uri()
        .query()
        .is_some_and(crate::auth::personal_tokens::credential_in_query)
    {
        return axum::response::IntoResponse::into_response(ApiError::new(
            axum::http::StatusCode::UNAUTHORIZED,
            "credential_in_url",
            "Send keys and tokens in the Authorization header, not in the URL. Revoke the one you sent.",
        ));
    }
    next.run(req).await
}

async fn not_found() -> ApiError {
    ApiError::not_found()
}

pub fn router(state: AppState) -> Router {
    let admin = Router::new()
        .route(
            "/session",
            post(admin_auth::login).delete(admin_auth::logout),
        )
        .route("/session/mfa", post(admin_auth::login_mfa))
        .route(
            "/session/mfa/passkey",
            post(admin_auth::begin_login_passkey),
        )
        .route("/session/reauth", post(admin_auth::reauth))
        .route(
            "/session/reauth/passkey",
            post(admin_auth::begin_reauth_passkey),
        )
        .route(
            "/me/mfa/passkeys",
            get(admin_mfa::list_passkeys).post(admin_mfa::begin_passkey),
        )
        .route("/me/mfa/passkeys/confirm", post(admin_mfa::confirm_passkey))
        .route("/me/mfa/passkeys/{id}", delete(admin_mfa::remove_passkey))
        .route("/me", get(admin_auth::me).patch(admin_auth::update_me))
        .route("/me/password", post(admin_auth::change_password))
        .route("/me/sessions", get(admin_auth::list_sessions))
        .route("/me/sessions/{id}", delete(admin_auth::revoke_session))
        .route("/me/login-history", get(admin_auth::login_history))
        .route(
            "/me/mfa/totp",
            post(admin_mfa::begin_totp).delete(admin_mfa::disable_totp),
        )
        .route("/me/mfa/totp/confirm", post(admin_mfa::confirm_totp))
        .route(
            "/me/mfa/recovery-codes",
            post(admin_mfa::regenerate_recovery_codes),
        )
        .route("/users", get(admin_workspaces::list_users))
        .route(
            "/workspaces",
            get(admin_workspaces::list_workspaces).post(admin_workspaces::create_workspace),
        )
        .route("/workspaces/{ws}", get(admin_workspaces::get_workspace))
        .route("/workspaces/{ws}/schema", get(admin_templates::schema_info))
        .route(
            "/workspaces/{ws}/entries",
            get(admin_entries::list).post(admin_entries::create),
        )
        .route(
            "/workspaces/{ws}/entries/{id}",
            get(admin_entries::get).delete(admin_publishing::delete),
        )
        .route(
            "/workspaces/{ws}/entries/{id}/draft",
            put(admin_entries::save_draft),
        )
        .route(
            "/workspaces/{ws}/entries/{id}/publish",
            post(admin_publishing::publish),
        )
        .route(
            "/workspaces/{ws}/entries/{id}/unpublish",
            post(admin_publishing::unpublish),
        )
        .route(
            "/workspaces/{ws}/entries/{id}/versions",
            get(admin_publishing::versions),
        )
        .route(
            "/workspaces/{ws}/entries/{id}/versions/{vid}",
            get(admin_publishing::version),
        )
        .route(
            "/workspaces/{ws}/entries/{id}/versions/{vid}/erase-candidates",
            get(admin_publishing::erase_candidates),
        )
        .route(
            "/workspaces/{ws}/entries/{id}/versions/{vid}/erase",
            post(admin_publishing::erase),
        )
        .route(
            "/workspaces/{ws}/entries/{id}/translations",
            post(admin_translations::create),
        )
        .route(
            "/workspaces/{ws}/entries/{id}/translation",
            get(admin_translations::info),
        )
        .route(
            "/workspaces/{ws}/entries/{id}/url-history",
            get(admin_publishing::url_history),
        )
        .route(
            "/templates/{name}/deploy",
            post(admin_templates::deploy_template),
        )
        .route(
            "/workspaces/{ws}/members",
            get(admin_workspaces::list_members),
        )
        .route(
            "/workspaces/{ws}/members/{user}",
            get(admin_workspaces::get_member)
                .patch(admin_workspaces::change_member_role)
                .delete(admin_workspaces::remove_member),
        )
        .route(
            "/workspaces/{ws}/invitations",
            get(admin_invitations::list).post(admin_invitations::invite),
        )
        .route(
            "/workspaces/{ws}/invitations/{id}",
            delete(admin_invitations::revoke),
        )
        .route(
            "/invitations/accept",
            post(admin_invitations::accept_as_new),
        )
        .route(
            "/me/invitations/accept",
            post(admin_invitations::accept_as_member),
        )
        .route(
            "/password-resets",
            post(admin_resets::request_password_reset),
        )
        .route(
            "/password-resets/complete",
            post(admin_resets::complete_password_reset),
        )
        .route(
            "/mfa-resets/complete",
            post(admin_resets::complete_mfa_reset),
        )
        .route(
            "/workspaces/{ws}/members/{user}/password-reset",
            post(admin_resets::issue_password_reset_in_workspace),
        )
        .route(
            "/workspaces/{ws}/members/{user}/mfa-reset",
            post(admin_resets::issue_mfa_reset_in_workspace),
        )
        .route(
            "/users/{user}/password-reset",
            post(admin_resets::issue_password_reset),
        )
        .route(
            "/users/{user}/mfa-reset",
            post(admin_resets::issue_mfa_reset),
        )
        .route(
            "/me/tokens",
            get(admin_tokens::list).post(admin_tokens::create),
        )
        .route("/me/tokens/{id}", delete(admin_tokens::revoke))
        .route(
            "/workspaces/{ws}/api-keys",
            get(admin_api_keys::list).post(admin_api_keys::create),
        )
        .route(
            "/workspaces/{ws}/api-keys/{id}",
            delete(admin_api_keys::revoke),
        )
        .route(
            "/workspaces/{ws}/webhooks",
            get(admin_webhooks::list).post(admin_webhooks::create),
        )
        .route(
            "/workspaces/{ws}/webhooks/{id}",
            delete(admin_webhooks::delete),
        )
        .route(
            "/workspaces/{ws}/webhooks/{id}/test",
            post(admin_webhooks::test),
        )
        .route(
            "/workspaces/{ws}/webhooks/{id}/rotate-secret",
            post(admin_webhooks::rotate_secret),
        )
        .route(
            "/workspaces/{ws}/webhooks/{id}/deliveries",
            get(admin_webhooks::deliveries),
        )
        .route(
            "/workspaces/{ws}/media",
            get(admin_media::list).post(admin_media::upload).layer(
                axum::extract::DefaultBodyLimit::max(
                    usize::try_from(state.config.media.max_file_bytes).unwrap_or(usize::MAX),
                ),
            ),
        )
        .route(
            "/workspaces/{ws}/media/{id}",
            get(admin_media::get).delete(admin_media::delete),
        )
        .route(
            "/workspaces/{ws}/limits",
            get(admin_media::get_limits).put(admin_media::set_limits),
        )
        .fallback(not_found);
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/media/{ws}/{file}", get(admin_media::serve))
        .route("/ws/{ws}/graphql", get(delivery::get).post(delivery::post))
        .route("/ws/{ws}/graphql/queries", post(delivery::register))
        .nest("/api/admin", admin)
        .fallback(not_found)
        .with_state(state)
        .layer(axum::middleware::from_fn(reject_credentials_in_url))
        .layer(axum::middleware::map_response(security_headers))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins() {
        assert_eq!(
            origin_of("https://CMS.example.com/"),
            "https://cms.example.com"
        );
        assert_eq!(
            origin_of("https://cms.example.com:8443/admin?x"),
            "https://cms.example.com:8443"
        );
    }
}
