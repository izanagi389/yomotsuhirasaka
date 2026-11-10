//! 管理API: パスワードと多要素認証のリセット（Q100, Q104, Q117）。
//!
//! | メソッド | パス | 誰が |
//! |---|---|---|
//! | POST | /api/admin/password-resets | 誰でも（本人がメールでリセットを要求する。SMTPがあるときだけ送る） |
//! | POST | /api/admin/password-resets/complete | トークンを持つ人（新しいパスワードを決める） |
//! | POST | /api/admin/mfa-resets/complete | トークンを持つ人（多要素認証を解除する） |
//! | POST | /api/admin/workspaces/{ws}/members/{user}/password-reset | 管理者・サーバー管理者（多要素認証・再認証） |
//! | POST | /api/admin/workspaces/{ws}/members/{user}/mfa-reset | 同上 |
//! | POST | /api/admin/users/{user}/password-reset | サーバー管理者（多要素認証・再認証） |
//! | POST | /api/admin/users/{user}/mfa-reset | 同上 |
//!
//! - 本人の要求への応答は、メールアドレスが登録されているかどうかで変えない（INV-AUTH-16）。
//! - ワークスペースの管理者が発行できるのは、自分が管理者であるワークスペースにしか所属していない利用者だけ（INV-AUTH-14）。
//! - サーバー管理者のリセットは、どの経路でも発行も完了もできない（INV-AUTH-10）。サーバー上のCLIで行う。
//! - 管理者は新しいパスワードを直接は決められない。本人向けのリンクを発行するだけ。
//!   SMTPがあればメールで送り、なければ発行した人に一度だけ渡す（Q117, Q193）。
//! - 完了したら、その利用者のセッションをすべて失効させる。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AppState;
use super::error::ApiError;
use super::extract::{AuthSession, ClientIp, JsonBody, check_origin};
use crate::access;
use crate::audit::{self, action};
use crate::auth::{mfa, password, session, users};
use crate::locale::Locale;
use crate::mail::{self, notify::Event};
use crate::resets::{self, Kind};

fn parse_id(raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|_| ApiError::not_found())
}

fn invalid_token() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "invalid_token",
        "The reset link is invalid or has expired.",
    )
}

fn locale_of(state: &AppState, stored: Option<&str>) -> Locale {
    stored
        .and_then(|l| l.parse().ok())
        .unwrap_or(state.config.default_locale)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestReset {
    email: String,
}

#[derive(Serialize)]
pub struct Accepted {
    status: &'static str,
}

/// 本人の要求の応答にかける時間（登録があるときの処理より十分に長く）。
const RESET_REQUEST_MIN_TIME: std::time::Duration = std::time::Duration::from_millis(300);

/// 本人がメールでパスワードのリセットを要求する。応答は、登録の有無・SMTPの有無によらず同じ。
pub async fn request_password_reset(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    JsonBody(req): JsonBody<RequestReset>,
) -> Result<(StatusCode, Json<Accepted>), ApiError> {
    // 登録の有無で応答の時間が変わらないよう、どの経路も同じ時間だけ待ってから返す
    let answer_at = tokio::time::Instant::now() + RESET_REQUEST_MIN_TIME;
    let result = handle_password_reset_request(&state, ip, &headers, req).await;
    tokio::time::sleep_until(answer_at).await;
    result
}

async fn handle_password_reset_request(
    state: &AppState,
    ip: std::net::IpAddr,
    headers: &HeaderMap,
    req: RequestReset,
) -> Result<(StatusCode, Json<Accepted>), ApiError> {
    check_origin(headers, &state.public_origin)?;
    state
        .login_limiter
        .check(ip)
        .map_err(ApiError::rate_limited)?;
    let accepted = (StatusCode::ACCEPTED, Json(Accepted { status: "accepted" }));
    if state.config.smtp.is_none() {
        return Ok(accepted);
    }
    let Some(email) = users::normalize_email(&req.email) else {
        return Ok(accepted);
    };
    let row: Option<(Uuid, bool, bool, Option<String>, bool)> = sqlx::query_as(
        "SELECT id, disabled, is_server_admin, locale, \
                (password_hash IS NULL AND EXISTS (SELECT 1 FROM yomi.external_identities x WHERE x.user_id = u.id)) \
         FROM yomi.users u WHERE email = $1",
    )
    .bind(&email)
    .fetch_optional(&state.pool)
    .await
    .map_err(anyhow::Error::from)?;
    let Some((user_id, disabled, server_admin, locale, sso_only)) = row else {
        return Ok(accepted);
    };
    // 無効にされた利用者と、サーバー管理者（ネットワーク越しには回復させない）と、
    // SSOだけでログインする人（IdPから外された後に、メールだけでパスワードを作らせない）には送らない
    if disabled || server_admin || sso_only {
        return Ok(accepted);
    }
    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    if resets::recently_issued(&mut *tx, user_id, Kind::Password).await? {
        return Ok(accepted);
    }
    let issued = resets::issue_alongside(
        &mut tx,
        user_id,
        Kind::Password,
        None,
        state.config.session.one_time_token_lifetime_seconds,
    )
    .await?;
    let link = resets::link(&state.config.public_url, Kind::Password, &issued.token);
    let message = mail::reset(
        &email,
        &state.issuer,
        Kind::Password,
        &link,
        &issued.expires_at,
        locale_of(state, locale.as_deref()),
    );
    mail::enqueue(&mut *tx, &state.keys, state.config.smtp.as_ref(), &message).await?;
    audit::record(
        &mut *tx,
        None,
        None,
        action::PASSWORD_RESET_REQUESTED,
        &[user_id],
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(accepted)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompletePasswordReset {
    token: String,
    new_password: String,
}

/// リセットのリンクから、新しいパスワードを決める。パスワードの規則に合わないときは、トークンを使わずに断る。
pub async fn complete_password_reset(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    JsonBody(req): JsonBody<CompletePasswordReset>,
) -> Result<StatusCode, ApiError> {
    check_origin(&headers, &state.public_origin)?;
    state
        .login_limiter
        .check(ip)
        .map_err(ApiError::rate_limited)?;
    let Some(preview) = resets::find_valid(&state.pool, &req.token, Kind::Password, false).await?
    else {
        return Err(invalid_token());
    };
    let words = password::personal_words(&preview.email, &state.config.public_url);
    let refs: Vec<&str> = words.iter().map(String::as_str).collect();
    if let Err(weakness) = password::check_policy(&req.new_password, &refs) {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "weak_password",
            weakness.message(),
        ));
    }
    super::admin_auth::reject_breached(&state, &req.new_password).await?;
    let hash = password::hash(req.new_password).await?;

    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    let Some(valid) = resets::find_valid(&mut *tx, &req.token, Kind::Password, true).await? else {
        return Err(invalid_token());
    };
    resets::mark_used(&mut tx, valid.id).await?;
    users::set_password(&mut *tx, valid.user_id, &hash).await?;
    users::cut_off_after_recovery(&mut tx, valid.user_id).await?;
    let notice = mail::notify::mail(
        &valid.email,
        &state.issuer,
        Event::PasswordChanged,
        locale_of(&state, valid.locale.as_deref()),
    );
    mail::enqueue(&mut *tx, &state.keys, state.config.smtp.as_ref(), &notice).await?;
    audit::record(
        &mut *tx,
        None,
        Some(valid.user_id),
        action::PASSWORD_RESET_COMPLETED,
        &[valid.user_id, valid.id],
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompleteMfaReset {
    token: String,
}

/// 多要素認証のリセットのリンクから、多要素認証を解除する。本人はパスワードでログインし、設定し直す。
pub async fn complete_mfa_reset(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    JsonBody(req): JsonBody<CompleteMfaReset>,
) -> Result<StatusCode, ApiError> {
    check_origin(&headers, &state.public_origin)?;
    state
        .login_limiter
        .check(ip)
        .map_err(ApiError::rate_limited)?;
    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    let Some(valid) = resets::find_valid(&mut *tx, &req.token, Kind::Mfa, true).await? else {
        return Err(invalid_token());
    };
    resets::mark_used(&mut tx, valid.id).await?;
    mfa::disable(&mut tx, valid.user_id).await?;
    users::cut_off_after_recovery(&mut tx, valid.user_id).await?;
    let notice = mail::notify::mail(
        &valid.email,
        &state.issuer,
        Event::MfaDisabled,
        locale_of(&state, valid.locale.as_deref()),
    );
    mail::enqueue(&mut *tx, &state.keys, state.config.smtp.as_ref(), &notice).await?;
    audit::record(
        &mut *tx,
        None,
        Some(valid.user_id),
        action::MFA_RESET_COMPLETED,
        &[valid.user_id, valid.id],
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuedReset {
    expires_at: String,
    /// "email": メールで本人に送った / "link": 応答のリンクを発行した人が本人に渡す（SMTPの設定がないとき）。
    delivery: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reset_url: Option<String>,
}

fn server_admin_target() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "server_admin_target",
        "Server administrators can only be recovered with the server CLI.",
    )
}

/// 管理者によるリセットの発行。`workspace` は、ワークスペースの管理者として発行するときのワークスペース。
async fn issue_reset(
    state: &AppState,
    current: &session::Current,
    workspace: Option<Uuid>,
    target: Uuid,
    kind: Kind,
) -> Result<(StatusCode, Json<IssuedReset>), ApiError> {
    match workspace {
        Some(ws) => {
            access::workspace_admin(&state.pool, current, ws).await??;
            // 対象がこのワークスペースに所属していなければ、存在しないものと同じ（INV-TEN-04）
            if access::role_in(&state.pool, ws, target).await?.is_none() {
                return Err(ApiError::not_found());
            }
            // サーバー管理者として操作しているなら範囲の制限はない。ワークスペースの管理者には範囲がある（INV-AUTH-14）
            if !access::acts_as_server_admin(current)
                && !access::reset_within_scope(&state.pool, current.user_id, target).await?
            {
                return Err(ApiError::new(
                    StatusCode::FORBIDDEN,
                    "outside_scope",
                    "This person also belongs to a workspace you do not administer. Ask a server administrator.",
                ));
            }
        }
        None => access::server_admin(current)?,
    }
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    let by_server_admin = access::acts_as_server_admin(current);
    // SMTPがなければ、リンクは発行した人に渡る。リンクを受け取った人は対象の人になりすませるので、
    // もともとすべての権限を持つサーバー管理者だけが発行できる
    if state.config.smtp.is_none() && !by_server_admin {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "smtp_required",
            "Email delivery (SMTP) is not configured, so only server administrators can issue reset links.",
        ));
    }
    let row: Option<(String, bool, Option<String>, bool)> = sqlx::query_as(
        "SELECT email, is_server_admin, locale, \
                (password_hash IS NULL AND EXISTS (SELECT 1 FROM yomi.external_identities x WHERE x.user_id = u.id)) \
         FROM yomi.users u WHERE id = $1",
    )
    .bind(target)
    .fetch_optional(&state.pool)
    .await
    .map_err(anyhow::Error::from)?;
    let Some((email, target_is_server_admin, locale, sso_only)) = row else {
        return Err(ApiError::not_found());
    };
    if target_is_server_admin {
        return Err(server_admin_target());
    }
    // SSOだけでログインする人にパスワードを作らせると、IdPから外された後も入れてしまう。
    // IdPで回復してもらう。どうしても要るときはサーバー管理者が発行する
    if kind == Kind::Password && sso_only && !by_server_admin {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "sso_only_user",
            "This person signs in only with SSO. Recover access through the identity provider, or ask a server administrator.",
        ));
    }

    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    let issued = resets::issue(
        &mut tx,
        target,
        kind,
        Some(current.user_id),
        state.config.session.one_time_token_lifetime_seconds,
    )
    .await?;
    let link = resets::link(&state.config.public_url, kind, &issued.token);
    let message = mail::reset(
        &email,
        &state.issuer,
        kind,
        &link,
        &issued.expires_at,
        locale_of(state, locale.as_deref()),
    );
    let mailed = mail::enqueue(&mut *tx, &state.keys, state.config.smtp.as_ref(), &message).await?;
    let what = match kind {
        Kind::Password => action::PASSWORD_RESET_ISSUED,
        Kind::Mfa => action::MFA_RESET_ISSUED,
    };
    // 管理者への通知（計画の15番）: 管理者のパスワードのリセット（多要素認証のリセットはいつでも知らせる。audit::NOTIFY）
    let admin_in = |ws: Uuid| {
        let pool = state.pool.clone();
        async move {
            Ok::<bool, ApiError>(
                access::role_in(&pool, ws, target).await? == Some(crate::access::Role::Admin),
            )
        }
    };
    let notify_in_workspace = match workspace {
        Some(ws) => kind == Kind::Password && admin_in(ws).await?,
        None => false,
    };
    audit::record_notify(
        &mut *tx,
        workspace,
        Some(current.user_id),
        what,
        &[target],
        notify_in_workspace,
    )
    .await?;
    // サーバー管理者がインスタンスの操作として発行したときも、対象が所属するワークスペースの監査ログに残す（Q122、INV-AUD-05）
    if workspace.is_none() {
        let member_of: Vec<Uuid> =
            sqlx::query_scalar("SELECT workspace_id FROM yomi.memberships WHERE user_id = $1")
                .bind(target)
                .fetch_all(&mut *tx)
                .await
                .map_err(anyhow::Error::from)?;
        for ws in member_of {
            let notify = kind == Kind::Password && admin_in(ws).await?;
            audit::record_notify(
                &mut *tx,
                Some(ws),
                Some(current.user_id),
                what,
                &[target],
                notify,
            )
            .await?;
        }
    }
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok((
        StatusCode::CREATED,
        Json(IssuedReset {
            expires_at: issued.expires_at,
            delivery: if mailed { "email" } else { "link" },
            reset_url: (!mailed).then_some(link),
        }),
    ))
}

pub async fn issue_password_reset_in_workspace(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, user)): Path<(String, String)>,
) -> Result<(StatusCode, Json<IssuedReset>), ApiError> {
    let (ws, user) = (parse_id(&ws)?, parse_id(&user)?);
    issue_reset(&state, &current, Some(ws), user, Kind::Password).await
}

pub async fn issue_mfa_reset_in_workspace(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, user)): Path<(String, String)>,
) -> Result<(StatusCode, Json<IssuedReset>), ApiError> {
    let (ws, user) = (parse_id(&ws)?, parse_id(&user)?);
    issue_reset(&state, &current, Some(ws), user, Kind::Mfa).await
}

pub async fn issue_password_reset(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(user): Path<String>,
) -> Result<(StatusCode, Json<IssuedReset>), ApiError> {
    issue_reset(&state, &current, None, parse_id(&user)?, Kind::Password).await
}

pub async fn issue_mfa_reset(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(user): Path<String>,
) -> Result<(StatusCode, Json<IssuedReset>), ApiError> {
    issue_reset(&state, &current, None, parse_id(&user)?, Kind::Mfa).await
}
