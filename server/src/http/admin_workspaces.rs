//! 管理API: ワークスペースと所属（Q76, Q101, Q121, Q122）。
//!
//! | メソッド | パス | 内容 | 誰が |
//! |---|---|---|---|
//! | GET | /api/admin/workspaces | 自分の所属するワークスペース（サーバー管理者はすべて） | ログイン中の人 |
//! | POST | /api/admin/workspaces | ワークスペースの作成 | サーバー管理者（多要素認証・再認証） |
//! | GET | /api/admin/workspaces/{ws} | ワークスペースの情報 | 所属者・サーバー管理者 |
//! | GET | /api/admin/workspaces/{ws}/members | 所属者の一覧 | 管理者・サーバー管理者（多要素認証）、閲覧の権限を与えられた所属者（Q190） |
//! | GET | /api/admin/workspaces/{ws}/members/{user} | 所属者の1人 | 同上 |
//! | PATCH | /api/admin/workspaces/{ws}/members/{user} | ロールと閲覧の権限の変更 | 管理者・サーバー管理者（多要素認証・再認証） |
//! | DELETE | /api/admin/workspaces/{ws}/members/{user} | 所属から外す | 同上・再認証 |
//! | GET | /api/admin/users | インスタンスの利用者の一覧 | サーバー管理者（多要素認証） |
//!
//! 所属していないワークスペースや、そのワークスペースに所属していない利用者は、存在しないものと同じ404にする（INV-TEN-04）。
//! 所属者を加えるのは招待の承諾だけ（Q121。`admin_invitations.rs`）。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AppState;
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::access::{self, Role};
use crate::audit::{self, action};
use crate::auth::session::Current;
use crate::workspace_db;

const MAX_NAME_CHARS: usize = 100;
/// テンプレートを指定しないときの名前（テンプレートのデプロイは後の段階で追加する）。
pub const DEFAULT_TEMPLATE: &str = "default";

fn parse_id(raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|_| ApiError::not_found())
}

const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInfo {
    id: Uuid,
    name: String,
    template: String,
    created_at: String,
    /// 自分のロール（所属していないサーバー管理者は null）。
    role: Option<String>,
}

type WorkspaceRow = (Uuid, String, String, String, Option<String>);

fn workspace_info((id, name, template, created_at, role): WorkspaceRow) -> WorkspaceInfo {
    WorkspaceInfo {
        id,
        name,
        template,
        created_at,
        role,
    }
}

pub async fn list_workspaces(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
) -> Result<Json<Vec<WorkspaceInfo>>, ApiError> {
    let rows: Vec<WorkspaceRow> = sqlx::query_as(crate::sql!(
        "SELECT w.id, w.name, w.template, to_char(w.created_at AT TIME ZONE 'UTC', {RFC3339}), m.role
         FROM cms.workspaces w
         LEFT JOIN cms.memberships m ON m.workspace_id = w.id AND m.user_id = $1
         WHERE m.user_id IS NOT NULL OR $2
         ORDER BY w.created_at, w.id"
    ))
    .bind(current.user_id)
    .bind(current.is_server_admin)
    .fetch_all(&state.pool)
    .await
    .map_err(anyhow::Error::from)?;
    Ok(Json(rows.into_iter().map(workspace_info).collect()))
}

async fn fetch_workspace(
    state: &AppState,
    current: &Current,
    id: Uuid,
) -> Result<WorkspaceInfo, ApiError> {
    let row: WorkspaceRow = sqlx::query_as(crate::sql!(
        "SELECT w.id, w.name, w.template, to_char(w.created_at AT TIME ZONE 'UTC', {RFC3339}), m.role
         FROM cms.workspaces w
         LEFT JOIN cms.memberships m ON m.workspace_id = w.id AND m.user_id = $2
         WHERE w.id = $1"
    ))
    .bind(id)
    .bind(current.user_id)
    .fetch_one(&state.pool)
    .await
    .map_err(anyhow::Error::from)?;
    Ok(workspace_info(row))
}

pub async fn get_workspace(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<WorkspaceInfo>, ApiError> {
    let id = parse_id(&ws)?;
    access::member(&state.pool, &current, id).await??;
    Ok(Json(fetch_workspace(&state, &current, id).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateWorkspace {
    name: String,
    template: Option<String>,
}

fn valid_template(t: &str) -> bool {
    (1..=64).contains(&t.len())
        && t.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// ワークスペースの作成。サーバー管理者だけ（INV-TEN-06）。スキーマとロールを、移行・ロールの作成用の接続で作る。
pub async fn create_workspace(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    JsonBody(req): JsonBody<CreateWorkspace>,
) -> Result<(StatusCode, Json<WorkspaceInfo>), ApiError> {
    access::server_admin(&current)?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    let name = req.name.trim();
    if name.is_empty()
        || name.chars().count() > MAX_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        return Err(ApiError::bad_request(format!(
            "`name` must be 1 to {MAX_NAME_CHARS} characters without control characters."
        )));
    }
    let template = req.template.as_deref().unwrap_or(DEFAULT_TEMPLATE);
    if !valid_template(template) {
        return Err(ApiError::bad_request(
            "`template` must be 1 to 64 characters of a-z, 0-9 and -.",
        ));
    }
    let id = Uuid::new_v4();
    workspace_db::provision(&state.migration, &state.app_role, id, name, template).await?;
    audit::record(
        &state.pool,
        Some(id),
        Some(current.user_id),
        action::WORKSPACE_CREATED,
        &[id],
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(fetch_workspace(&state, &current, id).await?),
    ))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberInfo {
    user_id: Uuid,
    email: String,
    role: String,
    can_view_members: bool,
    mfa_enabled: bool,
    joined_at: String,
}

type MemberRow = (Uuid, String, String, bool, bool, String);

fn member_info(
    (user_id, email, role, can_view_members, mfa_enabled, joined_at): MemberRow,
) -> MemberInfo {
    MemberInfo {
        user_id,
        email,
        role,
        can_view_members,
        mfa_enabled,
        joined_at,
    }
}

fn member_sql(filter: &str) -> sqlx::AssertSqlSafe<String> {
    crate::sql!(
        "SELECT u.id, u.email, m.role, m.can_view_members, u.totp_enabled_at IS NOT NULL,
                to_char(m.created_at AT TIME ZONE 'UTC', {RFC3339})
         FROM cms.memberships m JOIN cms.users u ON u.id = m.user_id
         WHERE m.workspace_id = $1 {filter}
         ORDER BY m.created_at, u.id"
    )
}

pub async fn list_members(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<Vec<MemberInfo>>, ApiError> {
    let id = parse_id(&ws)?;
    access::member_list_viewer(&state.pool, &current, id).await??;
    let rows: Vec<MemberRow> = sqlx::query_as(member_sql(""))
        .bind(id)
        .fetch_all(&state.pool)
        .await
        .map_err(anyhow::Error::from)?;
    Ok(Json(rows.into_iter().map(member_info).collect()))
}

async fn fetch_member(state: &AppState, ws: Uuid, user: Uuid) -> Result<MemberInfo, ApiError> {
    let row: Option<MemberRow> = sqlx::query_as(member_sql("AND m.user_id = $2"))
        .bind(ws)
        .bind(user)
        .fetch_optional(&state.pool)
        .await
        .map_err(anyhow::Error::from)?;
    // 所属していない利用者は、存在しない利用者と区別しない（INV-TEN-04）
    row.map(member_info).ok_or_else(ApiError::not_found)
}

pub async fn get_member(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, user)): Path<(String, String)>,
) -> Result<Json<MemberInfo>, ApiError> {
    let (ws, user) = (parse_id(&ws)?, parse_id(&user)?);
    access::member_list_viewer(&state.pool, &current, ws).await??;
    Ok(Json(fetch_member(&state, ws, user).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangeMember {
    role: Option<String>,
    /// 所属者の一覧の閲覧の権限（Q190）。
    can_view_members: Option<bool>,
}

fn last_admin() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "last_admin",
        "A workspace must keep at least one administrator.",
    )
}

/// 管理者を外す変更なら、他に管理者が残ることを確かめる（同時の変更に備えて、管理者の行をロックする）。
async fn ensure_other_admin_remains(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ws: Uuid,
    user: Uuid,
) -> Result<(), ApiError> {
    let admins: Vec<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM cms.memberships WHERE workspace_id = $1 AND role = 'admin' FOR UPDATE",
    )
    .bind(ws)
    .fetch_all(&mut **tx)
    .await
    .map_err(anyhow::Error::from)?;
    if admins.contains(&user) && admins.len() == 1 {
        return Err(last_admin());
    }
    Ok(())
}

/// ロールの変更（重い操作: 管理者・多要素認証・再認証。監査ログに記録）。
pub async fn change_member_role(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, user)): Path<(String, String)>,
    JsonBody(req): JsonBody<ChangeMember>,
) -> Result<Json<MemberInfo>, ApiError> {
    let (ws, user) = (parse_id(&ws)?, parse_id(&user)?);
    access::workspace_admin(&state.pool, &current, ws).await??;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    if req.role.is_none() && req.can_view_members.is_none() {
        return Err(ApiError::bad_request(
            "Send `role`, `canViewMembers` or both.",
        ));
    }
    let role: Option<Role> = req
        .role
        .as_deref()
        .map(str::parse)
        .transpose()
        .map_err(|()| ApiError::bad_request("`role` must be admin, editor, author or viewer."))?;
    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    if role.is_some_and(|r| r != Role::Admin) {
        ensure_other_admin_remains(&mut tx, ws, user).await?;
    }
    let done = sqlx::query(
        "UPDATE cms.memberships
         SET role = coalesce($3, role), can_view_members = coalesce($4, can_view_members)
         WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(ws)
    .bind(user)
    .bind(role.map(Role::as_str))
    .bind(req.can_view_members)
    .execute(&mut *tx)
    .await
    .map_err(anyhow::Error::from)?;
    if done.rows_affected() != 1 {
        return Err(ApiError::not_found());
    }
    for (changed, what) in [
        (role.is_some(), action::MEMBER_ROLE_CHANGED),
        (
            req.can_view_members.is_some(),
            action::MEMBER_PERMISSION_CHANGED,
        ),
    ] {
        if changed {
            audit::record(&mut *tx, Some(ws), Some(current.user_id), what, &[user]).await?;
        }
    }
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(fetch_member(&state, ws, user).await?))
}

/// 所属から外す（重い操作: 管理者・多要素認証・再認証。監査ログに記録）。
pub async fn remove_member(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, user)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (ws, user) = (parse_id(&ws)?, parse_id(&user)?);
    access::workspace_admin(&state.pool, &current, ws).await??;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    ensure_other_admin_remains(&mut tx, ws, user).await?;
    let done = sqlx::query("DELETE FROM cms.memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(ws)
        .bind(user)
        .execute(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
    if done.rows_affected() != 1 {
        return Err(ApiError::not_found());
    }
    audit::record(
        &mut *tx,
        Some(ws),
        Some(current.user_id),
        action::MEMBER_REMOVED,
        &[user],
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInfo {
    id: Uuid,
    email: String,
    is_server_admin: bool,
    disabled: bool,
    mfa_enabled: bool,
}

/// インスタンスの利用者の一覧（サーバー管理者だけ）。
pub async fn list_users(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
) -> Result<Json<Vec<UserInfo>>, ApiError> {
    access::server_admin(&current)?;
    let rows: Vec<(Uuid, String, bool, bool, bool)> = sqlx::query_as(
        "SELECT id, email, is_server_admin, disabled, totp_enabled_at IS NOT NULL
         FROM cms.users ORDER BY email",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(anyhow::Error::from)?;
    Ok(Json(
        rows.into_iter()
            .map(
                |(id, email, is_server_admin, disabled, mfa_enabled)| UserInfo {
                    id,
                    email,
                    is_server_admin,
                    disabled,
                    mfa_enabled,
                },
            )
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates() {
        assert!(valid_template("default"));
        assert!(valid_template("my-blog-2"));
        for bad in ["", "Blog", "a_b", "a b", &"a".repeat(65)] {
            assert!(!valid_template(bad), "{bad:?}");
        }
    }
}
