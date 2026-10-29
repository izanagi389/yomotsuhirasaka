//! 監査ログ（Q80, Q81, Q122）。追記だけを行う（普段用のロールには UPDATE と DELETE の権限がない）。
//!
//! 記録するのは、操作の種類・操作した人・対象のIDだけ。記事の本文や質問文などの内容は書かない（INV-AUD-01）。

use anyhow::Result;
use serde::Serialize;
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

/// 操作の種類。名前は外部への書き出し（M2）でもそのまま使うので、変えない。
pub mod action {
    pub const LOGIN: &str = "auth.login";
    /// 外部のIdP（OIDC）のアカウントをひも付けた（本人の操作か、招待の承諾）。
    pub const IDENTITY_LINKED: &str = "auth.identity_linked";
    /// SSOのログインで、ドメインのルール（JIT）により利用者を作った。
    pub const USER_PROVISIONED: &str = "auth.user_provisioned";
    /// 本人が表示名を変えた（名前そのものは書かない）。
    pub const DISPLAY_NAME_CHANGED: &str = "user.display_name_changed";
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
    pub const PERSONAL_TOKEN_ISSUED: &str = "personal_token.issued";
    pub const PERSONAL_TOKEN_REVOKED: &str = "personal_token.revoked";
    pub const API_KEY_ISSUED: &str = "api_key.issued";
    pub const API_KEY_REVOKED: &str = "api_key.revoked";
    /// 緊急停止: すべてのセッションと個人トークンの一斉失効（サーバー上のCLI）。
    pub const EMERGENCY_SESSIONS_REVOKED: &str = "emergency.sessions_revoked";
    /// 緊急停止: 配信APIのキーの入れ替え（サーバー上のCLI）。
    pub const API_KEYS_ROTATED: &str = "api_key.rotated";
    /// 暗号化の鍵の入れ替え（サーバー上のCLI。Q93）。
    pub const ENCRYPTION_KEY_ADDED: &str = "encryption_key.added";
    pub const SECRETS_REENCRYPTED: &str = "encryption_key.secrets_reencrypted";
    pub const ENCRYPTION_KEY_REMOVED: &str = "encryption_key.removed";
    /// テンプレートのデプロイ（インスタンス）と、ワークスペースごとの移行の結果。
    pub const TEMPLATE_DEPLOYED: &str = "template.deployed";
    pub const SCHEMA_MIGRATED: &str = "schema.migrated";
    pub const SCHEMA_MIGRATION_FAILED: &str = "schema.migration_failed";
    /// 所属していないサーバー管理者による記事の作成と下書きの保存（Q122）。
    pub const ENTRY_CREATED: &str = "entry.created";
    pub const ENTRY_DRAFT_SAVED: &str = "entry.draft_saved";
    pub const ENTRY_PUBLISHED: &str = "entry.published";
    /// 訂正としての公開（重い操作として、誰が行っても記録する）。
    pub const ENTRY_PUBLISHED_AS_CORRECTION: &str = "entry.published_as_correction";
    pub const ENTRY_UNPUBLISHED: &str = "entry.unpublished";
    pub const ENTRY_DELETED: &str = "entry.deleted";
    pub const ENTRY_SETTINGS_CHANGED: &str = "entry.settings_changed";
    /// 関連記事の固定・除外の変更（M3）。
    pub const ENTRY_RELATED_CHANGED: &str = "entry.related_changed";
    /// 「確認済み」の記録（Q46）。
    pub const ENTRY_VERIFIED: &str = "entry.verified";
    /// レビュー依頼（計画の8番）。対象は記事と依頼のID。差し戻しのコメントは書かない。
    pub const REVIEW_REQUESTED: &str = "review.requested";
    /// 予約公開（Q78）。対象は記事と予約のID。訂正による自動の取り消しは分けて記録する（Q133）。
    pub const ENTRY_SCHEDULED: &str = "entry.scheduled";
    /// 編集を引き継いだ（同時編集、Q73）。対象は記事と、引き継がれた人のID。
    pub const EDIT_TAKEN_OVER: &str = "entry.edit_taken_over";
    /// 原文の更新を反映済みにした（Q164）。
    pub const TRANSLATION_SOURCE_REFLECTED: &str = "translation.source_reflected";
    /// 翻訳の元の記事を設定し直した（Q156）。
    pub const TRANSLATION_SOURCE_CHANGED: &str = "translation.source_changed";
    /// ワークスペースの設定（編集の引き継ぎを誰ができるか）を変えた。
    pub const WORKSPACE_SETTINGS_CHANGED: &str = "workspace.settings_changed";
    pub const SCHEDULE_CANCELLED: &str = "entry.schedule_cancelled";
    pub const SCHEDULE_CANCELLED_BY_CORRECTION: &str = "entry.schedule_cancelled_by_correction";
    pub const SCHEDULE_FAILED: &str = "entry.schedule_failed";
    pub const REVIEW_REVIEWER_CHANGED: &str = "review.reviewer_changed";
    pub const REVIEW_REJECTED: &str = "review.rejected";
    pub const REVIEW_WITHDRAWN: &str = "review.withdrawn";
    /// 訂正の説明の記入・書き直し（Q63）。
    pub const CORRECTION_NOTE_EDITED: &str = "correction.note_edited";
    pub const CORRECTION_TEMPLATES_CHANGED: &str = "correction.templates_changed";
    /// 既存のブログからの取り込み（対象は取り込んだ記事）。
    pub const ENTRIES_IMPORTED: &str = "entry.imported";
    /// 版の本文の抹消（重い操作として、誰が行っても記録する。対象は記事・版・削除したメディア）。
    pub const VERSION_ERASED: &str = "version.erased";
    pub const MEDIA_UPLOADED: &str = "media.uploaded";
    pub const MEDIA_DELETED: &str = "media.deleted";
    pub const WORKSPACE_LIMITS_CHANGED: &str = "workspace.limits_changed";
    /// ワークスペースのLLMの予算の上限を変えた（重い権限 manageLlm）。
    pub const BUDGET_CHANGED: &str = "budget.changed";
    /// 予算が足りずにLLMの呼び出しを断った（期間ごとに1回。ワークスペースの管理者に知らせる）。
    pub const BUDGET_EXHAUSTED: &str = "budget.exhausted";
    /// 埋め込みモデルの切り替えを始めた・再開した（サーバー管理者。インスタンス全体の記録）。
    pub const EMBEDDING_SWITCH_STARTED: &str = "embedding_switch.started";
    /// 関連記事の一括再計算を今すぐ始めた（M3。公開の操作と同じく、サーバー管理者が操作したときだけ）。
    pub const RELATED_RECOMPUTE_STARTED: &str = "related.recompute_started";
    pub const WEBHOOK_CREATED: &str = "webhook.created";
    /// 監査ログの書き出し先（ワークスペース）の作成・削除・再開（計画の14番）。
    pub const AUDIT_SINK_CREATED: &str = "audit_sink.created";
    pub const AUDIT_SINK_DELETED: &str = "audit_sink.deleted";
    pub const AUDIT_SINK_RESUMED: &str = "audit_sink.resumed";
    /// 管理者への通知（計画の15番）: ログイン失敗の急増・回数制限への到達の急増（インスタンス単位）。
    pub const ALERT_LOGIN_FAILURES: &str = "alert.login_failures";
    pub const ALERT_RATE_LIMITED: &str = "alert.rate_limited";
    /// サーバー管理者の作成（サーバー上のCLI。Q100）。
    pub const SERVER_ADMIN_CREATED: &str = "server_admin.created";
    pub const WEBHOOK_DELETED: &str = "webhook.deleted";
    pub const WEBHOOK_SECRET_ROTATED: &str = "webhook.secret_rotated";
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
    record_full(
        executor,
        workspace_id,
        actor_user_id,
        action,
        target_ids,
        NOTIFY.contains(&action),
        None,
    )
    .await
}

/// 管理者に知らせる操作（13-security.md「管理者への通知」、Q174、計画の15番）。どの記録でも知らせるもの。
/// 管理者のロールへの変更・重い権限の付与・管理者のパスワードのリセットは、場合によるので記録する側が決める（`record_notify`）。
pub const NOTIFY: &[&str] = &[
    action::ALERT_LOGIN_FAILURES,
    action::ALERT_RATE_LIMITED,
    action::SERVER_ADMIN_CREATED,
    action::EMERGENCY_SESSIONS_REVOKED,
    action::API_KEYS_ROTATED,
    action::ENCRYPTION_KEY_ADDED,
    action::ENCRYPTION_KEY_REMOVED,
    action::MFA_RESET_ISSUED,
    action::MFA_RESET_COMPLETED,
    action::AUDIT_SINK_DELETED,
    action::BUDGET_EXHAUSTED,
];

/// 知らせるかを記録する側が決める記録。
pub async fn record_notify<'e>(
    executor: impl PgExecutor<'e>,
    workspace_id: Option<Uuid>,
    actor_user_id: Option<Uuid>,
    action: &str,
    target_ids: &[Uuid],
    notify: bool,
) -> Result<()> {
    record_full(
        executor,
        workspace_id,
        actor_user_id,
        action,
        target_ids,
        notify || NOTIFY.contains(&action),
        None,
    )
    .await
}

/// 記録する。`details` は急増の通知（alert.*。インスタンス単位）だけが使う（13-security.md、計画の15番）。
pub async fn record_full<'e>(
    executor: impl PgExecutor<'e>,
    workspace_id: Option<Uuid>,
    actor_user_id: Option<Uuid>,
    action: &str,
    target_ids: &[Uuid],
    notify: bool,
    details: Option<&serde_json::Value>,
) -> Result<()> {
    let targets: Vec<String> = target_ids.iter().map(Uuid::to_string).collect();
    sqlx::query(
        "INSERT INTO cms.audit_log (id, workspace_id, actor_user_id, action, target_ids, notify, details)
         VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb)",
    )
    .bind(Uuid::new_v4())
    .bind(workspace_id)
    .bind(actor_user_id)
    .bind(action)
    .bind(targets)
    .bind(notify)
    .bind(details.map(serde_json::Value::to_string))
    .execute(executor)
    .await?;
    Ok(())
}

/// 読む範囲（11-permissions-and-audit.md「閲覧の範囲」）。
#[derive(Debug, Clone, Copy)]
pub enum Scope {
    /// 1つのワークスペースの記録（そのワークスペースの管理者とサーバー管理者が読む）。
    Workspace(Uuid),
    /// インスタンス全体の記録（サーバー管理者だけが読む）。
    Instance,
}

/// 読み出した1件。記録した値だけを返す（内容は記録していない。INV-AUD-01）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub actor_user_id: Option<Uuid>,
    pub action: String,
    pub target_ids: Vec<String>,
    pub created_at: String,
    /// 管理者に知らせる記録か（計画の15番）。
    pub notify: bool,
    /// 急増の通知の詳細（件数・期間・件数の多いIPアドレスや利用者）。それ以外の記録は null。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// 外部への書き出しとダウンロードの1行（内容は入らない。INV-AUD-01）。名前は受け手が使うので変えない。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Line<'a> {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub id: Uuid,
    pub at: &'a str,
    pub workspace_id: Option<Uuid>,
    pub actor_user_id: Option<Uuid>,
    pub action: &'a str,
    pub target_ids: &'a [String],
    /// 管理者に知らせる記録か。
    pub notify: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<&'a serde_json::Value>,
}

/// 記録を1行の形にする。
pub fn line(e: &Entry) -> Line<'_> {
    Line {
        kind: "audit",
        id: e.id,
        at: &e.created_at,
        workspace_id: e.workspace_id,
        actor_user_id: e.actor_user_id,
        action: &e.action,
        target_ids: &e.target_ids,
        notify: e.notify,
        details: e.details.as_ref(),
    }
}

/// 読み出す列（`Row` の順）。表の別名は `a`。
pub const COLUMNS: &str = r#"a.id, a.workspace_id, a.actor_user_id, a.action, a.target_ids,
       to_char(a.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'), a.notify, a.details::text"#;

/// 読み出した行（ID, ワークスペース, 操作した人, 操作の種類, 対象, 時刻, 通知か, 詳細）。
pub type Row = (
    Uuid,
    Option<Uuid>,
    Option<Uuid>,
    String,
    Vec<String>,
    String,
    bool,
    Option<String>,
);

/// 行を記録にする。
pub fn entry(row: Row) -> Entry {
    let (id, workspace_id, actor_user_id, action, target_ids, created_at, notify, details) = row;
    Entry {
        id,
        workspace_id,
        actor_user_id,
        action,
        target_ids,
        created_at,
        notify,
        details: details.and_then(|d| serde_json::from_str(&d).ok()),
    }
}

/// 絞り込み（どれも省略できる）。時刻は確かめ済みの RFC 3339。
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub action: Option<String>,
    pub actor_user_id: Option<Uuid>,
    /// この時刻から（含む）。
    pub from: Option<String>,
    /// この時刻まで（含まない）。
    pub to: Option<String>,
}

/// 新しい順に読む。`before` を渡すと、その記録より古いものから読む（同じ範囲の記録のIDに限る）。
/// 範囲の外の `before` は、範囲の中に無いのと同じに扱う（何も返さない）。
pub async fn list(
    pool: &PgPool,
    scope: Scope,
    filter: &Filter,
    before: Option<Uuid>,
    limit: i64,
) -> Result<Vec<Entry>> {
    let workspace = match scope {
        Scope::Workspace(ws) => Some(ws),
        Scope::Instance => None,
    };
    let rows: Vec<Row> = sqlx::query_as(crate::sql!(
        r#"SELECT {COLUMNS}
           FROM cms.audit_log a
           WHERE a.workspace_id IS NOT DISTINCT FROM $1
             AND ($2::uuid IS NULL OR (a.created_at, a.id) < (
                   SELECT b.created_at, b.id FROM cms.audit_log b
                   WHERE b.id = $2 AND b.workspace_id IS NOT DISTINCT FROM $1))
             AND ($4::text IS NULL OR a.action = $4)
             AND ($5::uuid IS NULL OR a.actor_user_id = $5)
             AND ($6::timestamptz IS NULL OR a.created_at >= $6::timestamptz)
             AND ($7::timestamptz IS NULL OR a.created_at < $7::timestamptz)
           ORDER BY a.created_at DESC, a.id DESC
           LIMIT $3"#
    ))
    .bind(workspace)
    .bind(before)
    .bind(limit)
    .bind(&filter.action)
    .bind(filter.actor_user_id)
    .bind(&filter.from)
    .bind(&filter.to)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(entry).collect())
}
