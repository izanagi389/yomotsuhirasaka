//! 管理API: 外部のOIDC/SSOでのログイン・招待の承諾・ひも付け・再認証（13-security.md「OIDC/SSO」、Q103, Q108, Q126, Q131）。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | GET | /api/admin/session/oidc | SSOが使えるか、ボタンに出す名前 |
//! | POST | /api/admin/session/oidc/start | ログイン（`invitationToken` があれば招待の承諾）を始める |
//! | POST | /api/admin/me/identities/oidc/start | ログイン中の本人が、IdPのアカウントをひも付ける（再認証が必要） |
//! | POST | /api/admin/session/reauth/oidc | IdPでの再ログインによる再認証を始める（SSOのセッションで、管理者でない人だけ） |
//! | GET | /api/admin/session/oidc/callback | IdPからの戻り先 |
//! | POST | /api/admin/session/oidc/complete | SSOでログインした画面が、CSRFトークンを一度だけ受け取る |
//!
//! - 始めるときに、`state`・`nonce`・PKCE の値をサーバーに保存し、ブラウザには短命のCookie（SameSite=Lax）だけを渡す。
//!   戻ってきたら、Cookie と `state` の両方が一致した記録だけを使い、使ったら消す。
//! - メールアドレスが同じというだけで、既存のアカウントにひも付けない。ひも付けは、招待の承諾か、ログイン中の本人の操作だけ。
//! - アカウントの作成（JIT・招待）とひも付けは、`email_verified` が真のときだけ（INV-AUTH-19）。
//! - 戻り先では、結果を管理画面の `/admin/sso/complete` へのリダイレクトで伝える（失敗は `error` に種類だけ）。

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AppState;
use super::admin_auth::{Me, establish, set_login_cookies};
use super::error::ApiError;
use super::extract::{
    AuthSession, AuthSessionForMfaSetup, ClientIp, JsonBody, SESSION_COOKIE, check_origin, cookie,
    require_cookie_session,
};
use crate::audit::{self, action};
use crate::auth::oidc::{Oidc, OidcError, Verified};
use crate::auth::{session, token};
use crate::invitations;

/// ログインの途中で使う短命のCookie。IdPからのリダイレクト（別のサイトからの移動）で送られるよう、SameSite=Lax にする。
const FLOW_COOKIE: &str = "__Host-cms_oidc";
/// 始めてから戻るまでの期限（秒）。
const FLOW_SECONDS: u32 = 10 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Purpose {
    Login,
    Invite,
    Link,
    Reauth,
}

impl Purpose {
    fn as_str(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::Invite => "invite",
            Self::Link => "link",
            Self::Reauth => "reauth",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "login" => Self::Login,
            "invite" => Self::Invite,
            "link" => Self::Link,
            "reauth" => Self::Reauth,
            _ => return None,
        })
    }
}

fn oidc_not_configured() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "oidc_not_configured",
        "Single sign-on is not configured on this server.",
    )
}

fn oidc_unavailable() -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "oidc_unavailable",
        "Single sign-on is not available because outbound HTTP is not configured correctly.",
    )
}

fn idp_unreachable() -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "idp_unreachable",
        "The identity provider could not be reached or is not configured correctly.",
    )
}

fn oidc(state: &AppState) -> Result<&Oidc, ApiError> {
    state.oidc.as_deref().ok_or_else(oidc_not_configured)
}

fn flow_cookie(value: &str) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{FLOW_COOKIE}={value}; Path=/; Max-Age={FLOW_SECONDS}; Secure; HttpOnly; SameSite=Lax"
    ))
    .expect("token is URL-safe base64")
}

fn clear_flow_cookie() -> HeaderValue {
    HeaderValue::from_static("__Host-cms_oidc=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Lax")
}

/// PKCE の検証用の値を暗号化するときの、追加の認証データ。
fn pkce_aad(id: Uuid) -> Vec<u8> {
    format!("cms:oidc:{id}").into_bytes()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
}

/// SSOが使えるか（ログインの画面がボタンを出すかを決める）。
pub async fn info(State(state): State<AppState>) -> Json<Info> {
    Json(match state.oidc.as_deref() {
        Some(o) => Info {
            enabled: true,
            display_name: Some(o.display_name().to_owned()),
        },
        None => Info {
            enabled: false,
            display_name: None,
        },
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartedResponse {
    /// ブラウザを移動させる、IdPの認可のURL。
    authorization_url: String,
}

/// 認可を始め、途中の状態を保存して、IdPへのURLと短命のCookieを返す。
async fn begin(
    state: &AppState,
    purpose: Purpose,
    owner: Option<(Uuid, Uuid)>,
    invitation_id: Option<Uuid>,
) -> Result<Response, ApiError> {
    let oidc = oidc(state)?;
    let egress = state.egress.as_ref().ok_or_else(oidc_unavailable)?;
    let started = oidc
        .begin(egress, purpose == Purpose::Reauth)
        .await
        .map_err(|e| {
            tracing::warn!("OIDC: could not start sign-in: {e}");
            idp_unreachable()
        })?;
    let id = Uuid::new_v4();
    let browser = token::generate();
    let sealed = state
        .keys
        .encrypt(started.pkce_verifier.as_bytes(), &pkce_aad(id))?;
    // 期限を過ぎた途中の状態は、ここで消す
    sqlx::query("DELETE FROM cms.oidc_logins WHERE expires_at < now()")
        .execute(&state.pool)
        .await
        .map_err(anyhow::Error::from)?;
    sqlx::query(
        "INSERT INTO cms.oidc_logins
             (id, browser_hash, state_hash, nonce, pkce_verifier, purpose, user_id, session_id, invitation_id, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now() + make_interval(secs => $10))",
    )
    .bind(id)
    .bind(token::digest(&browser))
    .bind(token::digest(&started.state))
    .bind(&started.nonce)
    .bind(sealed)
    .bind(purpose.as_str())
    .bind(owner.map(|(user, _)| user))
    .bind(owner.map(|(_, session)| session))
    .bind(invitation_id)
    .bind(f64::from(FLOW_SECONDS))
    .execute(&state.pool)
    .await
    .map_err(anyhow::Error::from)?;
    let mut res = Json(StartedResponse {
        authorization_url: started.authorization_url,
    })
    .into_response();
    res.headers_mut()
        .append(header::SET_COOKIE, flow_cookie(&browser));
    Ok(res)
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartLogin {
    /// 招待のリンクのトークン。あれば、SSOで招待を承諾する（新しい利用者として）。
    #[serde(default)]
    invitation_token: Option<String>,
}

/// SSOでのログイン（または招待の承諾）を始める。
pub async fn start_login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    JsonBody(req): JsonBody<StartLogin>,
) -> Result<Response, ApiError> {
    check_origin(&headers, &state.public_origin)?;
    state
        .login_limiter
        .check(ip)
        .map_err(ApiError::rate_limited)?;
    oidc(&state)?;
    let (purpose, invitation_id) = match req.invitation_token {
        Some(t) => {
            let Some(invitation) = invitations::find_valid(&state.pool, &t, false).await? else {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "invalid_token",
                    "The invitation link is invalid or has expired.",
                ));
            };
            (Purpose::Invite, Some(invitation.id))
        }
        None => (Purpose::Login, None),
    };
    begin(&state, purpose, None, invitation_id).await
}

/// ログイン中の本人が、IdPのアカウントをひも付ける操作を始める。再認証が必要。
pub async fn start_link(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
) -> Result<Response, ApiError> {
    require_cookie_session(&current)?;
    oidc(&state)?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    begin(
        &state,
        Purpose::Link,
        Some((current.user_id, current.id)),
        None,
    )
    .await
}

/// IdPでの再ログインによる再認証を始める。SSOでログインした、管理者でない人だけ（管理者はパスキー。Q131）。
pub async fn start_reauth(
    State(state): State<AppState>,
    AuthSessionForMfaSetup(current): AuthSessionForMfaSetup,
) -> Result<Response, ApiError> {
    require_cookie_session(&current)?;
    oidc(&state)?;
    if !current.sso || current.admin_anywhere {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "reauth_method_not_allowed",
            "Confirm your identity with a passkey or your usual second factor instead.",
        ));
    }
    begin(
        &state,
        Purpose::Reauth,
        Some((current.user_id, current.id)),
        None,
    )
    .await
}

/// 途中の状態（使ったら消したもの）。
struct Flow {
    id: Uuid,
    nonce: String,
    pkce_verifier: Vec<u8>,
    purpose: Purpose,
    user_id: Option<Uuid>,
    session_id: Option<Uuid>,
    invitation_id: Option<Uuid>,
    created_at: i64,
}

/// 戻り先での失敗。管理画面には種類（`code`）だけを伝える。
#[derive(Debug)]
enum Fail {
    Code(&'static str),
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for Fail {
    fn from(e: anyhow::Error) -> Self {
        Self::Internal(e)
    }
}

impl From<sqlx::Error> for Fail {
    fn from(e: sqlx::Error) -> Self {
        Self::Internal(e.into())
    }
}

impl From<ApiError> for Fail {
    fn from(_: ApiError) -> Self {
        Self::Code("internal")
    }
}

/// `state` と短命のCookieの両方が一致する、期限内の途中の状態を取り出して消す（一度だけ使える）。
async fn take_flow(
    state: &AppState,
    state_param: &str,
    browser: &str,
) -> Result<Option<Flow>, Fail> {
    type Row = (
        Uuid,
        String,
        Vec<u8>,
        String,
        Option<Uuid>,
        Option<Uuid>,
        Option<Uuid>,
        i64,
    );
    let row: Option<Row> = sqlx::query_as(
        "DELETE FROM cms.oidc_logins
         WHERE state_hash = $1 AND browser_hash = $2 AND expires_at > now()
         RETURNING id, nonce, pkce_verifier, purpose, user_id, session_id, invitation_id,
                   floor(extract(epoch FROM created_at))::bigint",
    )
    .bind(token::digest(state_param))
    .bind(token::digest(browser))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.and_then(
        |(id, nonce, pkce_verifier, purpose, user_id, session_id, invitation_id, created_at)| {
            Some(Flow {
                id,
                nonce,
                pkce_verifier,
                purpose: Purpose::parse(&purpose)?,
                user_id,
                session_id,
                invitation_id,
                created_at,
            })
        },
    ))
}

/// 戻り先の処理の結果。ログインと招待の承諾では、新しいセッションを発行する。
enum Done {
    SignedIn(Box<super::admin_auth::Established>),
    Linked,
    Reauthenticated,
}

/// 管理画面の、SSOの結果を受け取るページのURL。
fn complete_page(state: &AppState, purpose: Option<Purpose>, error: Option<&str>) -> String {
    let mut url = format!(
        "{}/admin/sso/complete",
        state.config.public_url.trim_end_matches('/')
    );
    let mut sep = '?';
    if let Some(p) = purpose {
        url.push(sep);
        url.push_str("purpose=");
        url.push_str(p.as_str());
        sep = '&';
    }
    if let Some(code) = error {
        // code は下の固定の値だけ（利用者やIdPの入力ではない）
        url.push(sep);
        url.push_str("error=");
        url.push_str(code);
    }
    url
}

fn redirect(location: &str) -> Response {
    let mut res = StatusCode::SEE_OTHER.into_response();
    if let Ok(v) = HeaderValue::from_str(location) {
        res.headers_mut().insert(header::LOCATION, v);
    }
    res
}

/// IdPからの戻り先。結果は管理画面へのリダイレクトで伝える。
pub async fn callback(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let mut purpose = None;
    let outcome = handle_callback(&state, ip, &headers, &params, &mut purpose).await;
    let mut res = match outcome {
        Ok(done) => {
            let mut res = redirect(&complete_page(&state, purpose, None));
            if let Done::SignedIn(established) = &done {
                set_login_cookies(&state, &mut res, established);
            }
            res
        }
        Err(Fail::Code(code)) => redirect(&complete_page(&state, purpose, Some(code))),
        Err(Fail::Internal(e)) => {
            tracing::error!("OIDC callback failed: {e:#}");
            redirect(&complete_page(&state, purpose, Some("internal")))
        }
    };
    res.headers_mut()
        .append(header::SET_COOKIE, clear_flow_cookie());
    res
}

fn finish_error(e: OidcError) -> Fail {
    tracing::warn!("OIDC: sign-in rejected: {e}");
    match e {
        OidcError::InvalidIdToken => Fail::Code("invalid_id_token"),
        OidcError::Unreachable(_) | OidcError::BadDiscovery | OidcError::TokenRejected => {
            Fail::Code("idp_error")
        }
    }
}

async fn handle_callback(
    state: &AppState,
    ip: std::net::IpAddr,
    headers: &HeaderMap,
    params: &HashMap<String, String>,
    purpose_out: &mut Option<Purpose>,
) -> Result<Done, Fail> {
    let oidc = state
        .oidc
        .as_deref()
        .ok_or(Fail::Code("oidc_not_configured"))?;
    let egress = state.egress.as_ref().ok_or(Fail::Code("idp_error"))?;
    let browser = cookie(headers, FLOW_COOKIE).ok_or(Fail::Code("invalid_state"))?;
    let state_param = params.get("state").ok_or(Fail::Code("invalid_state"))?;
    let flow = take_flow(state, state_param, &browser)
        .await?
        .ok_or(Fail::Code("invalid_state"))?;
    *purpose_out = Some(flow.purpose);
    // IdPが断った・利用者が取りやめた
    if params.contains_key("error") {
        return Err(Fail::Code("cancelled"));
    }
    let code = params.get("code").ok_or(Fail::Code("invalid_request"))?;
    let verifier = state
        .keys
        .decrypt(&flow.pkce_verifier, &pkce_aad(flow.id))
        .ok()
        .and_then(|v| String::from_utf8(v).ok())
        .ok_or(Fail::Code("invalid_state"))?;
    let since = (flow.purpose == Purpose::Reauth).then_some(flow.created_at);
    let verified = oidc
        .finish(egress, code, verifier, &flow.nonce, since)
        .await
        .map_err(finish_error)?;

    match flow.purpose {
        Purpose::Login => login(state, oidc, ip, headers, &verified).await,
        Purpose::Invite => {
            let invitation_id = flow.invitation_id.ok_or(Fail::Code("invitation_invalid"))?;
            accept_invitation(state, ip, headers, &verified, invitation_id).await
        }
        Purpose::Link => {
            let (user_id, _) = owner(state, &flow).await?;
            link(state, &verified, user_id).await
        }
        Purpose::Reauth => {
            let (user_id, session_id) = owner(state, &flow).await?;
            reauthenticate(state, &verified, user_id, session_id).await
        }
    }
}

/// ひも付け・再認証を始めた本人と、そのセッション（まだ有効であること）。
async fn owner(state: &AppState, flow: &Flow) -> Result<(Uuid, Uuid), Fail> {
    let (Some(user_id), Some(session_id)) = (flow.user_id, flow.session_id) else {
        return Err(Fail::Code("invalid_state"));
    };
    if !session::is_active(&state.pool, session_id, user_id).await? {
        return Err(Fail::Code("session_expired"));
    }
    Ok((user_id, session_id))
}

/// (iss, sub) にひも付いた利用者（無効にされた利用者は除く）。
async fn find_identity(state: &AppState, v: &Verified) -> Result<Option<(Uuid, bool)>, Fail> {
    Ok(sqlx::query_as(
        "SELECT u.id, u.disabled FROM cms.external_identities x JOIN cms.users u ON u.id = x.user_id
         WHERE x.issuer = $1 AND x.subject = $2",
    )
    .bind(&v.issuer)
    .bind(&v.subject)
    .fetch_optional(&state.pool)
    .await?)
}

/// 確認済みのメールアドレス（`email_verified` が真のときだけ）。
fn verified_email(v: &Verified) -> Option<&str> {
    v.email.as_deref().filter(|_| v.email_verified)
}

async fn insert_identity<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    user_id: Uuid,
    v: &Verified,
) -> Result<Option<Uuid>, Fail> {
    let id = Uuid::new_v4();
    let done = sqlx::query(
        "INSERT INTO cms.external_identities (id, user_id, issuer, subject, last_login_at)
         VALUES ($1, $2, $3, $4, NULL)
         ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(user_id)
    .bind(&v.issuer)
    .bind(&v.subject)
    .execute(executor)
    .await?;
    Ok((done.rows_affected() == 1).then_some(id))
}

async fn touch_identity(state: &AppState, v: &Verified) -> Result<(), Fail> {
    sqlx::query(
        "UPDATE cms.external_identities SET last_login_at = now() WHERE issuer = $1 AND subject = $2",
    )
    .bind(&v.issuer)
    .bind(&v.subject)
    .execute(&state.pool)
    .await?;
    Ok(())
}

/// ログイン。ひも付いた利用者がいればログインさせる。
/// いなければ、`email_verified` が真でドメインがJITの対象のときだけ、新しい利用者を作る。
/// 同じメールアドレスの利用者がいても、ひも付けない。
async fn login(
    state: &AppState,
    oidc: &Oidc,
    ip: std::net::IpAddr,
    headers: &HeaderMap,
    v: &Verified,
) -> Result<Done, Fail> {
    if let Some((user_id, disabled)) = find_identity(state, v).await? {
        if disabled {
            return Err(Fail::Code("not_registered"));
        }
        touch_identity(state, v).await?;
        return Ok(Done::SignedIn(Box::new(
            establish(state, user_id, ip, headers, true).await?,
        )));
    }
    // ひも付けがない: JITの条件を満たさなければ、どの場合も同じ応答にする（アカウントの有無を推測させない）
    let Some(email) = verified_email(v).filter(|e| oidc.jit_allows(e)) else {
        return Err(Fail::Code("not_registered"));
    };
    let mut tx = state.pool.begin().await?;
    let user_id = Uuid::new_v4();
    let inserted = sqlx::query(
        "INSERT INTO cms.users (id, email) VALUES ($1, $2) ON CONFLICT (email) DO NOTHING",
    )
    .bind(user_id)
    .bind(email)
    .execute(&mut *tx)
    .await?;
    if inserted.rows_affected() != 1 {
        return Err(Fail::Code("not_registered"));
    }
    let Some(identity_id) = insert_identity(&mut *tx, user_id, v).await? else {
        return Err(Fail::Code("already_linked"));
    };
    audit::record(
        &mut *tx,
        None,
        Some(user_id),
        action::USER_PROVISIONED,
        &[user_id, identity_id],
    )
    .await?;
    tx.commit().await?;
    touch_identity(state, v).await?;
    Ok(Done::SignedIn(Box::new(
        establish(state, user_id, ip, headers, true).await?,
    )))
}

/// 招待の承諾（新しい利用者として）。招待の宛先と、確認済みのメールアドレスが同じであること。
async fn accept_invitation(
    state: &AppState,
    ip: std::net::IpAddr,
    headers: &HeaderMap,
    v: &Verified,
    invitation_id: Uuid,
) -> Result<Done, Fail> {
    let Some(email) = verified_email(v) else {
        return Err(Fail::Code("email_not_verified"));
    };
    if find_identity(state, v).await?.is_some() {
        return Err(Fail::Code("already_linked"));
    }
    let mut tx = state.pool.begin().await?;
    let Some(invitation) = invitations::find_valid_by_id_for_update(&mut tx, invitation_id).await?
    else {
        return Err(Fail::Code("invitation_invalid"));
    };
    if invitation.email != email {
        return Err(Fail::Code("email_mismatch"));
    }
    let user_id = Uuid::new_v4();
    let inserted = sqlx::query(
        "INSERT INTO cms.users (id, email) VALUES ($1, $2) ON CONFLICT (email) DO NOTHING",
    )
    .bind(user_id)
    .bind(email)
    .execute(&mut *tx)
    .await?;
    if inserted.rows_affected() != 1 {
        // 既にアカウントがある人は、ログインしてから承諾し、本人の操作でひも付ける
        return Err(Fail::Code("account_exists"));
    }
    let Some(identity_id) = insert_identity(&mut *tx, user_id, v).await? else {
        return Err(Fail::Code("already_linked"));
    };
    invitations::accept(&mut tx, &invitation, user_id).await?;
    audit::record(
        &mut *tx,
        Some(invitation.workspace_id),
        Some(user_id),
        action::MEMBER_JOINED,
        &[user_id, invitation.id],
    )
    .await?;
    audit::record(
        &mut *tx,
        None,
        Some(user_id),
        action::IDENTITY_LINKED,
        &[user_id, identity_id],
    )
    .await?;
    tx.commit().await?;
    touch_identity(state, v).await?;
    Ok(Done::SignedIn(Box::new(
        establish(state, user_id, ip, headers, true).await?,
    )))
}

/// ログイン中の本人の操作によるひも付け。`email_verified` が真で、そのIdPのアカウントが誰にもひも付いていないこと。
/// 1人の利用者は、同じ発行者のアカウントを1つだけひも付ける。
async fn link(state: &AppState, v: &Verified, user_id: Uuid) -> Result<Done, Fail> {
    if verified_email(v).is_none() {
        return Err(Fail::Code("email_not_verified"));
    }
    let mut tx = state.pool.begin().await?;
    let Some(identity_id) = insert_identity(&mut *tx, user_id, v).await? else {
        return Err(Fail::Code("already_linked"));
    };
    audit::record(
        &mut *tx,
        None,
        Some(user_id),
        action::IDENTITY_LINKED,
        &[user_id, identity_id],
    )
    .await?;
    tx.commit().await?;
    Ok(Done::Linked)
}

/// IdPでの再ログインによる再認証。ひも付いた本人であること。
async fn reauthenticate(
    state: &AppState,
    v: &Verified,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<Done, Fail> {
    match find_identity(state, v).await? {
        Some((id, false)) if id == user_id => {}
        _ => return Err(Fail::Code("identity_mismatch")),
    }
    touch_identity(state, v).await?;
    session::mark_reauthenticated(&state.pool, session_id, session::Method::Oidc).await?;
    Ok(Done::Reauthenticated)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Completed {
    csrf_token: String,
    user: Me,
}

/// SSOでログインした画面が、CSRFトークンを一度だけ受け取る。
/// まだCSRFトークンを持っていないので、CSRFトークンは確かめない（セッションのCookieは SameSite=Strict、`Origin` も確かめる）。
pub async fn complete(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<impl Serialize>, ApiError> {
    check_origin(&headers, &state.public_origin)?;
    if headers.contains_key(header::AUTHORIZATION) {
        return Err(ApiError::unauthenticated());
    }
    let presented = cookie(&headers, SESSION_COOKIE).ok_or_else(ApiError::unauthenticated)?;
    let current = session::resolve(&state.pool, &presented, &state.config.session)
        .await?
        .ok_or_else(ApiError::unauthenticated)?;
    let Some(csrf_token) = session::take_csrf_handoff(&state.pool, current.id).await? else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "csrf_already_issued",
            "The CSRF token for this session was already issued. Sign in again if you lost it.",
        ));
    };
    Ok(Json(Completed {
        csrf_token,
        user: Me::new(&state, &current),
    }))
}
