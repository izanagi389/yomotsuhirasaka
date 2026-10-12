//! 監査ログ（Q80, Q81, Q122）。追記だけを行う（普段用のロールには UPDATE と DELETE の権限がない）。
//!
//! 記録するのは、操作の種類・操作した人・対象のIDだけ。記事の本文や質問文などの内容は書かない（INV-AUD-01）。

use anyhow::Result;
use sqlx::PgExecutor;
use uuid::Uuid;

/// 操作の種類。名前は外部への書き出し（M2）でもそのまま使うので、変えない。
pub mod action {
    pub const LOGIN: &str = "auth.login";
    pub const WORKSPACE_CREATED: &str = "workspace.created";
    pub const MEMBER_ROLE_CHANGED: &str = "member.role_changed";
    pub const MEMBER_REMOVED: &str = "member.removed";
    pub const MEMBER_PERMISSION_CHANGED: &str = "member.permission_changed";
    pub const MEMBER_INVITED: &str = "member.invited";
    pub const INVITATION_REVOKED: &str = "invitation.revoked";
    pub const MEMBER_JOINED: &str = "member.joined";
    pub const PASSWORD_RESET_REQUESTED: &str = "password_reset.requested";
    pub const PASSWORD_RESET_ISSUED: &str = "password_reset.issued";
    pub const PASSWORD_RESET_COMPLETED: &str = "password_reset.completed";
    pub const MFA_RESET_ISSUED: &str = "mfa_reset.issued";
    pub const MFA_RESET_COMPLETED: &str = "mfa_reset.completed";
}

/// 1件を記録する。`workspace_id` が None ならインスタンス全体の記録。
/// サーバー管理者がワークスペースのデータを操作したときは、そのワークスペースのIDを渡す（Q122）。
pub async fn record<'e>(
    executor: impl PgExecutor<'e>,
    workspace_id: Option<Uuid>,
    actor_user_id: Option<Uuid>,
    action: &str,
    target_ids: &[Uuid],
) -> Result<()> {
    let targets: Vec<String> = target_ids.iter().map(Uuid::to_string).collect();
    sqlx::query(
        "INSERT INTO cms.audit_log (id, workspace_id, actor_user_id, action, target_ids)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(workspace_id)
    .bind(actor_user_id)
    .bind(action)
    .bind(targets)
    .execute(executor)
    .await?;
    Ok(())
}
