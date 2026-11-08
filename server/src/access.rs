//! ワークスペースのロールと、操作の可否の判定（Q76, Q90, Q101, Q121, Q122）。
//!
//! - ロールはリクエストのたびに `yomi.memberships` と照合する（セッションに保存した値を信用しない）。
//! - 所属していないワークスペースは、存在しないものと同じに見せる（404）。
//! - サーバー管理者とワークスペースの管理者は、多要素認証を設定していないと管理操作ができない（INV-AUTH-05）。
//! - サーバー管理者は、所属していないワークスペースも操作できる。その操作はワークスペースの監査ログに残す（Q122）。
//! - 重い権限は、ロールの既定に加えて、管理者が個人に追加で付与できる（Q76）。付与もリクエストのたびに照合する。

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

/// 個人に追加で付与できる権限（11-permissions-and-audit.md「ロールと権限」、Q76, Q190）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// 所属者の一覧の閲覧（M1）。
    ViewMembers,
    /// 質問ログの閲覧。どのロールにも既定では付けない（INV-PERM-02）。
    ViewQuestionLog,
    /// LLMの接続設定と予算の変更。
    ManageLlm,
    /// サイトの方針に関わるスイッチ（学習への利用の拒否、外部AI向けの素材に含める、チャットボットの対象）。
    PolicySwitches,
    /// 訂正としての公開と、確認済みの記録。
    Corrections,
    /// 編集の引き継ぎ。
    TakeOverEditing,
}

impl Permission {
    pub const ALL: [Permission; 6] = [
        Permission::ViewMembers,
        Permission::ViewQuestionLog,
        Permission::ManageLlm,
        Permission::PolicySwitches,
        Permission::Corrections,
        Permission::TakeOverEditing,
    ];

    /// 重い権限（管理APIの `grants`・`permissions` で扱うもの。所属者の一覧の閲覧は `canViewMembers` で扱う）。
    pub fn is_heavy(self) -> bool {
        self != Permission::ViewMembers
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Permission::ViewMembers => "viewMembers",
            Permission::ViewQuestionLog => "viewQuestionLog",
            Permission::ManageLlm => "manageLlm",
            Permission::PolicySwitches => "policySwitches",
            Permission::Corrections => "corrections",
            Permission::TakeOverEditing => "takeOverEditing",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == s)
    }

    /// ロールが既定で持つか。
    pub fn role_default(self, role: Role) -> bool {
        match self {
            Permission::ViewMembers | Permission::ManageLlm | Permission::PolicySwitches => {
                role == Role::Admin
            }
            Permission::ViewQuestionLog => false,
            Permission::Corrections | Permission::TakeOverEditing => {
                matches!(role, Role::Admin | Role::Editor)
            }
        }
    }

    fn bit(self) -> u8 {
        1 << (self as u8)
    }
}

/// 個人に付与した権限の集まり。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Grants(u8);

impl Grants {
    /// DBの値（名前の配列）から。知らない名前は無視する（DBの制約で入らない）。
    pub fn from_names<S: AsRef<str>>(names: &[S]) -> Self {
        Grants(
            names
                .iter()
                .filter_map(|n| Permission::parse(n.as_ref()))
                .fold(0, |acc, p| acc | p.bit()),
        )
    }

    pub fn contains(self, p: Permission) -> bool {
        self.0 & p.bit() != 0
    }

    pub fn with(self, p: Permission, on: bool) -> Self {
        Grants(if on {
            self.0 | p.bit()
        } else {
            self.0 & !p.bit()
        })
    }

    /// DBに書く値（名前の配列）。
    pub fn names(self) -> Vec<&'static str> {
        Permission::ALL
            .into_iter()
            .filter(|p| self.contains(*p))
            .map(Permission::as_str)
            .collect()
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
    /// 個人に付与した権限（所属しているとき）。
    pub grants: Grants,
}

impl Access {
    /// 権限を持つか（ロールの既定か、個人への付与）。所属していないサーバー管理者は、質問ログの閲覧のほかはすべて持つ。
    pub fn has(&self, p: Permission) -> bool {
        if self.grants.contains(p) {
            return true;
        }
        match self.role {
            Some(role) => p.role_default(role),
            None => self.server_admin && p != Permission::ViewQuestionLog,
        }
    }

    /// 持っている重い権限の名前（管理画面が操作を出し分けるため）。
    pub fn heavy_permissions(&self) -> Vec<&'static str> {
        Permission::ALL
            .into_iter()
            .filter(|p| p.is_heavy() && self.has(*p))
            .map(Permission::as_str)
            .collect()
    }
}

/// 所属しているときのロールと、個人に付与した権限。
async fn membership(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Option<(Role, Grants)>> {
    let row: Option<(String, Vec<String>)> = sqlx::query_as(
        "SELECT role, grants FROM yomi.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|(r, g)| Some((r.parse().ok()?, Grants::from_names(&g)))))
}

pub async fn role_in(pool: &PgPool, workspace_id: Uuid, user_id: Uuid) -> Result<Option<Role>> {
    let role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM yomi.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(role.and_then(|r| r.parse().ok()))
}

async fn workspace_exists(pool: &PgPool, workspace_id: Uuid) -> Result<bool> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM yomi.workspaces WHERE id = $1)")
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
    let found = membership(pool, workspace_id, current.user_id).await?;
    let role = found.map(|(r, _)| r);
    if role.is_none() && !acts_as_server_admin(current) {
        return Ok(Err(Denied::NotFound));
    }
    Ok(Ok(Access {
        workspace_id,
        role,
        server_admin: acts_as_server_admin(current) && role.is_none(),
        grants: found.map(|(_, g)| g).unwrap_or_default(),
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
    if access.grants.contains(Permission::ViewMembers) {
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
             SELECT 1 FROM yomi.memberships t
             WHERE t.user_id = $2
               AND NOT EXISTS (SELECT 1 FROM yomi.memberships c
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

    #[test]
    fn permissions_round_trip_and_defaults() {
        for p in Permission::ALL {
            assert_eq!(Permission::parse(p.as_str()), Some(p));
        }
        // 質問ログの閲覧は、どのロールにも既定で付かない（INV-PERM-02）
        for r in [Role::Admin, Role::Editor, Role::Author, Role::Viewer] {
            assert!(!Permission::ViewQuestionLog.role_default(r));
        }
        let access = |role, grants| Access {
            workspace_id: Uuid::nil(),
            role,
            server_admin: role.is_none(),
            grants,
        };
        assert!(!access(Some(Role::Editor), Grants::default()).has(Permission::PolicySwitches));
        assert!(access(Some(Role::Editor), Grants::default()).has(Permission::Corrections));
        let g = Grants::default().with(Permission::Corrections, true);
        assert!(access(Some(Role::Author), g).has(Permission::Corrections));
        assert!(
            !access(Some(Role::Author), g.with(Permission::Corrections, false))
                .has(Permission::Corrections)
        );
        assert!(!access(None, Grants::default()).has(Permission::ViewQuestionLog));
        assert!(access(None, Grants::default()).has(Permission::ManageLlm));
        assert_eq!(Grants::from_names(&g.names()), g);
    }
}
