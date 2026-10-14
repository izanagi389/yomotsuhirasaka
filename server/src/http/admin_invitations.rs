//! 管理API: 招待（Q104, Q121, Q193）。
//!
//! | メソッド | パス | 内容 | 誰が |
//! |---|---|---|---|
//! | POST | /api/admin/workspaces/{ws}/invitations | 招待する | 管理者・サーバー管理者（多要素認証・再認証。SMTPがなければサーバー管理者だけ） |
//! | GET | /api/admin/workspaces/{ws}/invitations | 承諾待ちの招待の一覧 | 管理者・サーバー管理者（多要素認証） |
//! | DELETE | /api/admin/workspaces/{ws}/invitations/{id} | 招待を取り消す | 同上 |
//! | POST | /api/admin/invitations/accept | 新しい利用者として承諾する（パスワードを決める） | トークンを持つ人 |
//! | POST | /api/admin/me/invitations/accept | ログイン中の既存の利用者として承諾する | 招待の宛先の本人 |
//!
//! - 招待への応答は、相手が既存の利用者かどうかで変えない。
//! - SMTPを設定していれば、リンクはメールで送り、応答には含めない。設定していなければ、応答にリンクを一度だけ含める（Q193）。
//!   その場合に招待できるのはサーバー管理者だけ（Q194）。
//! - 使えないトークン（存在しない・期限切れ・使用済み・取り消し済み・宛先が違う）は、すべて同じ応答にする。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AppState;
use super::error::ApiError;
use super::extract::{AuthSession, ClientIp, JsonBody, check_origin, require_cookie_session};
use crate::access::{self, Role};
use crate::audit::{self, action};
use crate::auth::{password, users};
use crate::{invitations, mail};

fn parse_id(raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|_| ApiError::not_found())
}

fn invalid_token() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "invalid_token",
        "The invitation link is invalid or has expired.",
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InviteRequest {
    email: String,
    role: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
/// 招待の応答。相手が既存の利用者かどうかで変わらないよう、送られた値（メールアドレス）も返さない（INV-TEN-04）。
pub struct InviteResponse {
    id: Uuid,
    role: String,
    expires_at: String,
    /// "email": メールで送った / "link": 応答のリンクを管理者が本人に渡す（SMTPの設定がないとき）。
    delivery: &'static str,
    /// SMTPの設定がないときだけ、ここで一度だけ渡す。
    #[serde(skip_serializing_if = "Option::is_none")]
    invite_url: Option<String>,
}

fn already_member() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "already_member",
        "This person is already a member of the workspace.",
    )
}

pub async fn invite(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    JsonBody(req): JsonBody<InviteRequest>,
) -> Result<(StatusCode, Json<InviteResponse>), ApiError> {
    let ws = parse_id(&ws)?;
    access::workspace_admin(&state.pool, &current, ws).await??;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    // SMTPがないとリンクは招待した人の手に渡る。ワークスペースの管理者が自分で承諾を試すと、
    // 他のワークスペースの利用者の存在が分かってしまうので、サーバー管理者だけに許す（Q194、INV-TEN-04）
    if state.config.smtp.is_none() && !access::acts_as_server_admin(&current) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "smtp_required",
            "Invitations by workspace administrators require email (SMTP) to be configured. Ask a server administrator.",
        ));
    }
    let email = users::normalize_email(&req.email)
        .ok_or_else(|| ApiError::bad_request("`email` is not a valid email address."))?;
    let role: Role = req
        .role
        .parse()
        .map_err(|()| ApiError::bad_request("`role` must be admin, editor, author or viewer."))?;

    // 既に所属していれば知らせる（所属者は管理者が一覧で見られるので、新しい情報は漏れない）
    let member: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM cms.memberships m JOIN cms.users u ON u.id = m.user_id
                        WHERE m.workspace_id = $1 AND u.email = $2)",
    )
    .bind(ws)
    .bind(&email)
    .fetch_one(&state.pool)
    .await
    .map_err(anyhow::Error::from)?;
    if member {
        return Err(already_member());
    }
    let workspace_name: String =
        sqlx::query_scalar("SELECT name FROM cms.workspaces WHERE id = $1")
            .bind(ws)
            .fetch_one(&state.pool)
            .await
            .map_err(anyhow::Error::from)?;

    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    let created = invitations::create(
        &mut tx,
        ws,
        &email,
        role,
        current.user_id,
        state.config.session.one_time_token_lifetime_seconds,
    )
    .await?;
    audit::record(
        &mut *tx,
        Some(ws),
        Some(current.user_id),
        action::MEMBER_INVITED,
        &[created.id],
    )
    .await?;
    let link = invitations::link(&state.config.public_url, &created.token);
    // 宛先が既存の利用者で言語を設定していればその言語、なければインスタンスの既定の言語（Q198）。
    // メールの言語は招待した人には見えないので、相手が既存の利用者かどうかは漏れない
    let recipient_locale: Option<String> =
        sqlx::query_scalar("SELECT locale FROM cms.users WHERE email = $1")
            .bind(&email)
            .fetch_optional(&mut *tx)
            .await
            .map_err(anyhow::Error::from)?
            .flatten();
    let locale = recipient_locale
        .and_then(|l| l.parse().ok())
        .unwrap_or(state.config.default_locale);
    let message = mail::invitation(
        &email,
        &state.issuer,
        &workspace_name,
        &link,
        &created.expires_at,
        locale,
    );
    let mailed = mail::enqueue(&mut *tx, &state.keys, state.config.smtp.as_ref(), &message).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;

    Ok((
        StatusCode::CREATED,
        Json(InviteResponse {
            id: created.id,
            role: role.as_str().to_owned(),
            expires_at: created.expires_at,
            delivery: if mailed { "email" } else { "link" },
            invite_url: (!mailed).then_some(link),
        }),
    ))
}

pub async fn list(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<Vec<invitations::Summary>>, ApiError> {
    let ws = parse_id(&ws)?;
    access::workspace_admin(&state.pool, &current, ws).await??;
    Ok(Json(invitations::list_pending(&state.pool, ws).await?))
}

pub async fn revoke(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (ws, id) = (parse_id(&ws)?, parse_id(&id)?);
    access::workspace_admin(&state.pool, &current, ws).await??;
    if !invitations::revoke(&state.pool, ws, id).await? {
        return Err(ApiError::not_found());
    }
    audit::record(
        &state.pool,
        Some(ws),
        Some(current.user_id),
        action::INVITATION_REVOKED,
        &[id],
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcceptAsNew {
    token: String,
    new_password: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Accepted {
    user_id: Uuid,
    workspace_id: Uuid,
    role: String,
}

fn account_exists() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "account_exists",
        "An account with this email already exists. Sign in and accept the invitation.",
    )
}

/// 新しい利用者として承諾する。パスワードの規則に合わないときは、トークンを使わずに断る（直してやり直せる）。
pub async fn accept_as_new(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    JsonBody(req): JsonBody<AcceptAsNew>,
) -> Result<(StatusCode, Json<Accepted>), ApiError> {
    check_origin(&headers, &state.public_origin)?;
    state
        .login_limiter
        .check(ip)
        .map_err(ApiError::rate_limited)?;
    // パスワードのハッシュ（重い計算）の前に、トークンを確かめる
    let Some(preview) = invitations::find_valid(&state.pool, &req.token, false).await? else {
        return Err(invalid_token());
    };
    if users::find_for_login(&state.pool, &preview.email)
        .await?
        .is_some()
    {
        return Err(account_exists());
    }
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
    // 同時の承諾に備えて、ロックしてから確かめ直す
    let Some(invitation) = invitations::find_valid(&mut *tx, &req.token, true).await? else {
        return Err(invalid_token());
    };
    let user_id = Uuid::new_v4();
    let inserted = sqlx::query(
        "INSERT INTO cms.users (id, email, password_hash, password_changed_at)
         VALUES ($1, $2, $3, now())
         ON CONFLICT (email) DO NOTHING",
    )
    .bind(user_id)
    .bind(&invitation.email)
    .bind(&hash)
    .execute(&mut *tx)
    .await
    .map_err(anyhow::Error::from)?;
    if inserted.rows_affected() != 1 {
        return Err(account_exists());
    }
    invitations::accept(&mut tx, &invitation, user_id).await?;
    audit::record(
        &mut *tx,
        Some(invitation.workspace_id),
        Some(user_id),
        action::MEMBER_JOINED,
        &[user_id, invitation.id],
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok((
        StatusCode::CREATED,
        Json(Accepted {
            user_id,
            workspace_id: invitation.workspace_id,
            role: invitation.role.as_str().to_owned(),
        }),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcceptAsMember {
    token: String,
}

/// ログイン中の既存の利用者として承諾する。招待の宛先が本人でなければ、使えないトークンと同じ応答にする。
pub async fn accept_as_member(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    JsonBody(req): JsonBody<AcceptAsMember>,
) -> Result<Json<Accepted>, ApiError> {
    require_cookie_session(&current)?;
    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    let Some(invitation) = invitations::find_valid(&mut *tx, &req.token, true).await? else {
        return Err(invalid_token());
    };
    if invitation.email != current.email {
        return Err(invalid_token());
    }
    invitations::accept(&mut tx, &invitation, current.user_id).await?;
    audit::record(
        &mut *tx,
        Some(invitation.workspace_id),
        Some(current.user_id),
        action::MEMBER_JOINED,
        &[current.user_id, invitation.id],
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    let role = access::role_in(&state.pool, invitation.workspace_id, current.user_id)
        .await?
        .unwrap_or(invitation.role);
    Ok(Json(Accepted {
        user_id: current.user_id,
        workspace_id: invitation.workspace_id,
        role: role.as_str().to_owned(),
    }))
}
