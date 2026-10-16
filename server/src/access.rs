//! ワークスペースのロールと、操作の可否の判定（Q76, Q90, Q101, Q121, Q122）。
//!
//! - ロールはリクエストのたびに `cms.memberships` と照合する（セッションに保存した値を信用しない）。
//! - 所属していないワークスペースは、存在しないものと同じに見せる（404）。
//! - サーバー管理者とワークスペースの管理者は、多要素認証を設定していないと管理操作ができない（INV-AUTH-05）。
//! - サーバー管理者は、所属していないワークスペースも操作できる。その操作はワークスペースの監査ログに残す（Q122）。

use std::fmt;
use std::str::FromStr;

use anyhow::Result;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::session::Current;

/// ワークスペースの固定のロール。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Admin,
    Editor,
    Author,
    Viewer,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Editor => "editor",
            Role::Author => "author",
            Role::Viewer => "viewer",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Role {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "admin" => Ok(Role::Admin),
            "editor" => Ok(Role::Editor),
            "author" => Ok(Role::Author),
            "viewer" => Ok(Role::Viewer),
            _ => Err(()),
        }
    }
}

/// 操作を拒否する理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    /// 所属していない（存在を知らせない）。
    NotFound,
    /// 所属しているが、ロールが足りない。
    Forbidden,
    /// 管理操作に必要な多要素認証を設定していない。
    MfaRequired,
}

/// あるワークスペースに対する、ログイン中の利用者の立場。
#[derive(Debug, Clone, Copy)]
pub struct Access {
    pub workspace_id: Uuid,
    /// 所属しているときのロール。
    pub role: Option<Role>,
    /// サーバー管理者として操作しているか。
    pub server_admin: bool,
}

pub async fn role_in(pool: &PgPool, workspace_id: Uuid, user_id: Uuid) -> Result<Option<Role>> {
    let role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM cms.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(role.and_then(|r| r.parse().ok()))
}

async fn workspace_exists(pool: &PgPool, workspace_id: Uuid) -> Result<bool> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM cms.workspaces WHERE id = $1)")
            .bind(workspace_id)
            .fetch_one(pool)
            .await?,
    )
}

/// サーバー管理者として、所属していないワークスペースも操作できるか。
/// 個人トークンでは、サーバー管理者でもこの立場は取れない（トークンの権限は、本人の所属とロールの範囲だけ。Q119）。
pub fn acts_as_server_admin(current: &Current) -> bool {
    current.is_server_admin && !current.via_token()
}

/// 所属者（どのロールでも）かサーバー管理者なら通す。
pub async fn member(
    pool: &PgPool,
    current: &Current,
    workspace_id: Uuid,
) -> Result<Result<Access, Denied>> {
    if !workspace_exists(pool, workspace_id).await? {
        return Ok(Err(Denied::NotFound));
    }
    // 個人トークンは、発行時に選んだワークスペースでだけ使える（INV-AUTH-09）
    if let Some(grant) = &current.grant
        && !grant.workspaces.contains(&workspace_id)
    {
        return Ok(Err(Denied::NotFound));
    }
    let role = role_in(pool, workspace_id, current.user_id).await?;
    if role.is_none() && !acts_as_server_admin(current) {
        return Ok(Err(Denied::NotFound));
    }
    Ok(Ok(Access {
        workspace_id,
        role,
        server_admin: acts_as_server_admin(current) && role.is_none(),
    }))
}

/// ワークスペースの管理者かサーバー管理者で、多要素認証を設定していれば通す（SSOのセッションではCMS側のパスキー。Q131）。
pub async fn workspace_admin(
    pool: &PgPool,
    current: &Current,
    workspace_id: Uuid,
) -> Result<Result<Access, Denied>> {
    let access = match member(pool, current, workspace_id).await? {
        Ok(a) => a,
        Err(d) => return Ok(Err(d)),
    };
    if access.role != Some(Role::Admin) && !acts_as_server_admin(current) {
        return Ok(Err(Denied::Forbidden));
    }
    if !current.admin_mfa_satisfied() {
        return Ok(Err(Denied::MfaRequired));
    }
    Ok(Ok(access))
}

/// 所属者の一覧を見られる人なら通す（Q190）。
/// 管理者とサーバー管理者は管理操作として多要素認証を求める。閲覧の権限を与えられた所属者はそのまま通す。
pub async fn member_list_viewer(
    pool: &PgPool,
    current: &Current,
    workspace_id: Uuid,
) -> Result<Result<Access, Denied>> {
    let access = match member(pool, current, workspace_id).await? {
        Ok(a) => a,
        Err(d) => return Ok(Err(d)),
    };
    if access.role == Some(Role::Admin) || acts_as_server_admin(current) {
        if !current.admin_mfa_satisfied() {
            return Ok(Err(Denied::MfaRequired));
        }
        return Ok(Ok(access));
    }
    let granted: bool = sqlx::query_scalar(
        "SELECT can_view_members FROM cms.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(current.user_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or(false);
    if granted {
        Ok(Ok(access))
    } else {
        Ok(Err(Denied::Forbidden))
    }
}

/// ワークスペースの管理者が、利用者のリセットを発行できるか（Q117、INV-AUTH-14）。
/// 対象が所属するすべてのワークスペースで、発行する人が管理者であれば true。
pub async fn reset_within_scope(pool: &PgPool, issuer: Uuid, target: Uuid) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT NOT EXISTS (
             SELECT 1 FROM cms.memberships t
             WHERE t.user_id = $2
               AND NOT EXISTS (SELECT 1 FROM cms.memberships c
                               WHERE c.workspace_id = t.workspace_id AND c.user_id = $1 AND c.role = 'admin'))",
    )
    .bind(issuer)
    .bind(target)
    .fetch_one(pool)
    .await?)
}

/// サーバー管理者で、多要素認証を設定していれば通す。
pub fn server_admin(current: &Current) -> Result<(), Denied> {
    // インスタンス全体の操作は、個人トークンではできない（デプロイの範囲は、デプロイの機能とともに扱う）
    if !acts_as_server_admin(current) {
        return Err(Denied::Forbidden);
    }
    if !current.admin_mfa_satisfied() {
        return Err(Denied::MfaRequired);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_round_trip() {
        for r in [Role::Admin, Role::Editor, Role::Author, Role::Viewer] {
            assert_eq!(r.as_str().parse::<Role>(), Ok(r));
        }
        assert!("owner".parse::<Role>().is_err());
        assert!("Admin".parse::<Role>().is_err());
    }
}
