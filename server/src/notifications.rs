//! 管理者への通知の一覧と「確認した」の記録（13-security.md「管理者への通知」、計画の15番）。
//! 通知は、知らせる印（`audit_log.notify`）の付いた監査ログの記録。直近7日の分を、まだ確認していない人に見せる。

use anyhow::Result;
use sqlx::PgPool;
use uuid::Uuid;

use crate::audit::{self, Entry};

/// 見せる期間（日）。
pub const DAYS: i32 = 7;
/// 1回に見せる件数の上限。
const LIMIT: i64 = 100;

/// まだ確認していない通知（古い順）。workspace が None ならインスタンス単位の通知。
pub async fn unread(pool: &PgPool, workspace: Option<Uuid>, user: Uuid) -> Result<Vec<Entry>> {
    let rows: Vec<audit::Row> = sqlx::query_as(crate::sql!(
        "SELECT {}
         FROM cms.audit_log a
         WHERE a.notify AND a.workspace_id IS NOT DISTINCT FROM $1
           AND a.created_at > now() - make_interval(days => $2)
           AND NOT EXISTS (SELECT 1 FROM cms.notification_reads r WHERE r.user_id = $3 AND r.audit_id = a.id)
         ORDER BY a.created_at, a.id
         LIMIT $4",
        audit::COLUMNS
    ))
    .bind(workspace)
    .bind(DAYS)
    .bind(user)
    .bind(LIMIT)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(audit::entry).collect())
}

/// 確認したことを記録する（その範囲の通知でなければ false）。
pub async fn mark_read(
    pool: &PgPool,
    workspace: Option<Uuid>,
    user: Uuid,
    audit_id: Uuid,
) -> Result<bool> {
    let ok: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM cms.audit_log WHERE id = $1 AND notify AND workspace_id IS NOT DISTINCT FROM $2)",
    )
    .bind(audit_id)
    .bind(workspace)
    .fetch_one(pool)
    .await?;
    if !ok {
        return Ok(false);
    }
    sqlx::query("INSERT INTO cms.notification_reads (user_id, audit_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(user)
        .bind(audit_id)
        .execute(pool)
        .await?;
    Ok(true)
}

/// 見せる期間を十分に過ぎた「確認した」の記録を消す。
pub async fn sweep(pool: &PgPool) -> Result<()> {
    sqlx::query(
        "DELETE FROM cms.notification_reads WHERE read_at < now() - make_interval(days => $1)",
    )
    .bind(DAYS * 2)
    .execute(pool)
    .await?;
    Ok(())
}
