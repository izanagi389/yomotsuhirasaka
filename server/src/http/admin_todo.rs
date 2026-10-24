//! 管理API: 「やること」（03-content-and-editing.md「やることの見せ方（Q67）」、計画の12番、INV-TODO-01）。
//!
//! | メソッド | パス | 内容 | 誰が |
//! |---|---|---|---|
//! | GET | /api/admin/workspaces/{ws}/todo | その人が対応できる項目を、優先度の順に並べた一覧 | 所属者・サーバー管理者 |
//!
//! - 項目は、その人が対応する権限を持つものだけ（INV-TODO-01）。権限は呼ばれるたびに照合する（原則3）。
//!   - 予約のやり直し中・レビュー待ち: 公開できる人
//!   - 予約の失敗・訂正による取り消し: 予約した本人。差し戻し: 依頼した本人
//!   - 未記入の訂正・確認期限切れ: 「訂正と鮮度」の権限を持つ人
//!   - 原文の更新: 記事を編集できる人（執筆者以上）
//! - 優先度（上ほど先）: 予約の失敗・やり直し中 → 自分が指定されたレビュー待ち → その他のレビュー待ち → 未記入の訂正 →
//!   差し戻し → 原文の訂正 → 確認期限切れ → 原文の更新 → 訂正による予約の取り消し。同じ優先度の中では古いものが先。
//! - 読むだけで、監査ログには残さない。

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, State};
use serde::Serialize;
use uuid::Uuid;

use super::AppState;
use super::admin_entries::{db, parse, readable, reader};
use super::admin_reviews::{self, RequestView};
use super::admin_scheduling::{self, ScheduleView};
use super::error::ApiError;
use super::extract::AuthSession;
use crate::access::{Access, Permission, Role};
use crate::content::entries::{self, Summary};
use crate::content::{corrections, freshness, reviews, scheduling, translations};
use crate::workspace_db;

/// 項目の種類（宣言の順が優先度の順）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    ScheduleFailed,
    ScheduleRetrying,
    ReviewWaiting,
    UnwrittenCorrection,
    ReviewRejected,
    SourceCorrected,
    ReviewExpired,
    SourceUpdated,
    ScheduleCancelled,
}

impl Kind {
    /// 優先度（小さいほど先）。予約の失敗とやり直し中は同じ。
    fn priority(self) -> u8 {
        match self {
            Kind::ScheduleFailed | Kind::ScheduleRetrying => 0,
            Kind::ReviewWaiting => 1,
            Kind::UnwrittenCorrection => 2,
            Kind::ReviewRejected => 3,
            Kind::SourceCorrected => 4,
            Kind::ReviewExpired => 5,
            Kind::SourceUpdated => 6,
            Kind::ScheduleCancelled => 7,
        }
    }
}

/// 「やること」の1件。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    kind: Kind,
    entry: Summary,
    /// 並べるときに使う時刻（依頼・差し戻し・予約・訂正・確認期限・原文の公開の時刻）。
    at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    request: Option<RequestView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    schedule: Option<ScheduleView>,
    /// 未記入の訂正の版。
    #[serde(skip_serializing_if = "Option::is_none")]
    version_id: Option<Uuid>,
}

#[derive(Serialize)]
pub struct Todo {
    items: Vec<Item>,
}

fn can_publish(access: &Access) -> bool {
    access.server_admin || matches!(access.role, Some(Role::Admin | Role::Editor))
}

fn can_edit(access: &Access) -> bool {
    access.server_admin || matches!(access.role, Some(Role::Admin | Role::Editor | Role::Author))
}

/// 種類と記事と時刻だけの項目。
struct Raw {
    kind: Kind,
    entry_id: Uuid,
    at: String,
    request: Option<RequestView>,
    schedule: Option<ScheduleView>,
    version_id: Option<Uuid>,
}

impl Raw {
    fn new(kind: Kind, entry_id: Uuid, at: String) -> Self {
        Raw {
            kind,
            entry_id,
            at,
            request: None,
            schedule: None,
            version_id: None,
        }
    }
}

pub async fn todo(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<Todo>, ApiError> {
    let ws = parse(&ws)?;
    let access = reader(&state, &current, ws).await?;
    let applied = readable(&state, ws).await?;
    let publish = can_publish(&access);
    let corrections_holder = access.has(Permission::Corrections);
    let edit = can_edit(&access);

    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let retrying = if publish {
        scheduling::retrying(&mut tx).await?
    } else {
        Vec::new()
    };
    let attention = scheduling::needing_attention(&mut tx, current.user_id).await?;
    let waiting = if publish {
        reviews::waiting(&mut tx).await?
    } else {
        Vec::new()
    };
    let rejected = reviews::rejected_for(&mut tx, current.user_id).await?;
    let (unwritten, expired) = if corrections_holder {
        (
            corrections::unwritten(&mut tx, &applied.manifest).await?,
            freshness::expired(&mut tx, &applied.manifest).await?,
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let updated = if edit {
        translations::updated(&mut tx).await?
    } else {
        Vec::new()
    };
    let summaries: HashMap<Uuid, Summary> = entries::list(&mut tx, &applied.manifest)
        .await?
        .into_iter()
        .map(|s| (s.id, s))
        .collect();
    tx.commit().await.map_err(anyhow::Error::from)?;

    let mut raw = Vec::new();
    for s in admin_scheduling::views(&state, ws, retrying).await? {
        let mut r = Raw::new(
            Kind::ScheduleRetrying,
            s.schedule.entry_id,
            s.schedule.publish_at.clone(),
        );
        r.schedule = Some(s);
        raw.push(r);
    }
    for s in admin_scheduling::views(&state, ws, attention).await? {
        let kind = if s.schedule.state == "failed" {
            Kind::ScheduleFailed
        } else {
            Kind::ScheduleCancelled
        };
        let at = s
            .schedule
            .closed_at
            .clone()
            .unwrap_or_else(|| s.schedule.publish_at.clone());
        let mut r = Raw::new(kind, s.schedule.entry_id, at);
        r.schedule = Some(s);
        raw.push(r);
    }
    for v in admin_reviews::views(&state, ws, &current, waiting).await? {
        let mut r = Raw::new(Kind::ReviewWaiting, v.entry_id, v.created_at.clone());
        r.request = Some(v);
        raw.push(r);
    }
    for v in admin_reviews::views(&state, ws, &current, rejected).await? {
        let at = v.closed_at.clone().unwrap_or_else(|| v.created_at.clone());
        let mut r = Raw::new(Kind::ReviewRejected, v.entry_id, at);
        r.request = Some(v);
        raw.push(r);
    }
    for u in unwritten {
        let mut r = Raw::new(Kind::UnwrittenCorrection, u.entry_id, u.corrected_at);
        r.version_id = Some(u.version_id);
        raw.push(r);
    }
    for e in expired {
        raw.push(Raw::new(Kind::ReviewExpired, e.entry_id, e.review_due_at));
    }
    for u in updated {
        let kind = if u.corrected {
            Kind::SourceCorrected
        } else {
            Kind::SourceUpdated
        };
        raw.push(Raw::new(kind, u.entry_id, u.published_at));
    }

    // 優先度 → （レビュー待ちは）自分が指定されたものが先 → 古い順
    raw.sort_by(|a, b| {
        let mine = |r: &Raw| {
            !r.request
                .as_ref()
                .is_some_and(|q| q.assigned_to_me && r.kind == Kind::ReviewWaiting)
        };
        (a.kind.priority(), mine(a), &a.at, a.kind).cmp(&(
            b.kind.priority(),
            mine(b),
            &b.at,
            b.kind,
        ))
    });
    let items = raw
        .into_iter()
        .filter_map(|r| {
            // 削除された記事（一覧にない記事）の項目は出さない
            summaries.get(&r.entry_id).cloned().map(|entry| Item {
                kind: r.kind,
                entry,
                at: r.at,
                request: r.request,
                schedule: r.schedule,
                version_id: r.version_id,
            })
        })
        .collect();
    Ok(Json(Todo { items }))
}
