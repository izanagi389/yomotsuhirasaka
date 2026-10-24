//! 管理API: 予約公開（03-content-and-editing.md「予約公開（Q78）」、Q133、計画の9番）と、予約の時刻に公開するジョブ。
//!
//! | メソッド | パス | 内容 | 誰が |
//! |---|---|---|---|
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/schedule | 開いている予約と、最後の予約 | 所属者・サーバー管理者 |
//! | PUT | /api/admin/workspaces/{ws}/entries/{id}/schedule | 予約する・予約し直す（`{revision, publishAt}`） | 公開できる人 |
//! | DELETE | /api/admin/workspaces/{ws}/entries/{id}/schedule | 予約を取り消す | 公開できる人 |
//! | GET | /api/admin/workspaces/{ws}/schedules | 「要対応」用: やり直している予約（公開できる人に）と、自分の予約で訂正による取り消しか失敗になったもの | 所属者・サーバー管理者 |
//!
//! - 予約のときに、公開と同じ検査と計算（スキーマ、公開前のフック、URL生成関数、URLの重なり、本文の変換）を済ませて保存する。
//!   その後の下書きの編集は予約版に影響しない。「訂正として公開」は予約できない（INV-SCHED-02）。
//! - 予約の時刻にジョブが公開する。公開日時は実際に公開した日時。公開できなければ理由を残してジョブの間隔でやり直し、
//!   予約の時刻から24時間たったら失敗にする。
//! - 開いているレビュー依頼は、予約した時点で「予約済み」として閉じる。
//! - Webhook: `entry.scheduled`（予約の時刻）と `entry.schedule_cancelled`（理由）。予約版の新しいURLは送らない。

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AppState;
use super::admin_entries::{db, parse, readable, reader, writable};
use super::admin_publishing::{
    notify_webhooks, publish_events, publisher, refused, schedule_hooks,
};
use super::admin_reviews::{Person, publisher_people};
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::access::{Access, Role};
use crate::audit::{self, action};
use crate::auth::session::Current;
use crate::content::entries::{self, Summary};
use crate::content::publishing::{self, Refused, Scripts};
use crate::content::scheduling::{self, Reason, Schedule, TimeCheck};
use crate::content::{applied_schema, reviews};
use crate::jobs::{self, Outcome};
use crate::script::Host;
use crate::webhooks::{self, ScheduleEvent};
use crate::workspace_db;

/// 予約の時刻に公開するジョブの種類。
pub const JOB_KIND: &str = "scheduled_publish";
/// ジョブのやり直しの回数の上限（間隔は最大1時間なので、24時間を十分に超える）。
const MAX_ATTEMPTS: i32 = 64;

fn can_publish(access: &Access) -> bool {
    access.server_admin || matches!(access.role, Some(Role::Admin | Role::Editor))
}

/// 応答の予約。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleView {
    #[serde(flatten)]
    schedule: Schedule,
    /// 予約した人（いまも公開できる人のときだけ。それ以外は null）。
    scheduled_by_person: Option<Person>,
}

async fn views(
    state: &AppState,
    ws: Uuid,
    list: Vec<Schedule>,
) -> Result<Vec<ScheduleView>, ApiError> {
    let ids: Vec<Uuid> = list.iter().map(|s| s.scheduled_by).collect();
    let people = publisher_people(state, ws, &ids).await?;
    Ok(list
        .into_iter()
        .map(|s| ScheduleView {
            scheduled_by_person: people.get(&s.scheduled_by).cloned(),
            schedule: s,
        })
        .collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntrySchedule {
    /// 開いている予約（やり直している間も）。
    open: Option<ScheduleView>,
    /// 最後の予約（閉じたものも。取り消しや失敗を示すため）。
    latest: Option<ScheduleView>,
}

async fn entry_schedule(state: &AppState, ws: Uuid, id: Uuid) -> Result<EntrySchedule, ApiError> {
    let db = db(state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let open = scheduling::open(&mut tx, id).await?;
    let latest = scheduling::latest(&mut tx, id).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    // 最後の予約が開いている予約と同じなら、latest には入れない
    let latest = latest.filter(|l| open.as_ref().is_none_or(|o| o.id != l.id));
    let open = views(state, ws, open.into_iter().collect()).await?.pop();
    let latest = views(state, ws, latest.into_iter().collect()).await?.pop();
    Ok(EntrySchedule { open, latest })
}

pub async fn get_schedule(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<EntrySchedule>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    reader(&state, &current, ws).await?;
    Ok(Json(entry_schedule(&state, ws, id).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleRequest {
    /// 予約する下書きのリビジョン番号（読んだときの値）。
    revision: i64,
    /// 公開する時刻（RFC 3339）。
    publish_at: String,
    /// 訂正として公開の予約は受け付けない（送られたら断る）。
    #[serde(default)]
    correction: bool,
    #[serde(default)]
    correction_note: Option<serde_json::Value>,
}

fn invalid_time(message: &'static str) -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_publish_at",
        message,
    )
}

pub async fn put_schedule(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
    JsonBody(req): JsonBody<ScheduleRequest>,
) -> Result<Json<EntrySchedule>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    publisher(&state, &current, ws).await?;
    if req.correction || req.correction_note.is_some() {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "cannot_schedule_correction",
            "Publishing as a correction cannot be scheduled. Publish the correction now.",
        ));
    }
    if !scheduling::well_formed_time(&req.publish_at) {
        return Err(invalid_time(
            "`publishAt` must be an RFC 3339 date and time.",
        ));
    }
    let applied = writable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    match scheduling::check_time(&mut tx, &req.publish_at).await? {
        TimeCheck::Ok => {}
        TimeCheck::Invalid => {
            return Err(invalid_time("`publishAt` is not a valid date and time."));
        }
        TimeCheck::OutOfRange => {
            return Err(invalid_time(
                "`publishAt` must be in the future and within a year.",
            ));
        }
    }
    let scripts = Scripts {
        engine: &state.scripts,
        host: Host::new(state.pool.clone(), db.clone()),
    };
    let prepared =
        match publishing::prepare_scheduled(&mut tx, &applied.manifest, &scripts, id, req.revision)
            .await?
        {
            Ok(p) => p,
            Err(r) => return Err(refused(r)),
        };
    let schedule_id = Uuid::new_v4();
    let delay: f64 =
        sqlx::query_scalar("SELECT extract(epoch FROM $1::timestamptz - now())::float8")
            .bind(&req.publish_at)
            .fetch_one(&mut *tx)
            .await
            .map_err(anyhow::Error::from)?;
    let (_, created) = scheduling::create(
        &mut tx,
        schedule_id,
        id,
        &req.publish_at,
        req.revision,
        applied.version,
        &prepared,
        current.user_id,
    )
    .await?;
    reviews::close(
        &mut tx,
        id,
        reviews::State::Scheduled,
        current.user_id,
        None,
    )
    .await?;
    let entry = webhooks::entry_ref(&mut tx, id).await?;
    // 公開のジョブは、予約を確定する前に積む（確定できなければ、ジョブは予約を見つけずに何もしない）
    jobs::enqueue_debounced(
        &state.pool,
        &state.keys,
        Some(ws),
        JOB_KIND,
        &serde_json::to_vec(&Job {
            workspace_id: ws,
            schedule_id,
        })
        .map_err(anyhow::Error::from)?,
        MAX_ATTEMPTS,
        &format!("{JOB_KIND}:{schedule_id}"),
        delay.max(0.0),
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    if let Some(entry) = entry {
        let event = ScheduleEvent::Scheduled {
            at: &created.publish_at,
        };
        if let Err(e) = webhooks::emit_schedule(&state.pool, &state.keys, ws, &entry, event).await {
            tracing::error!(workspace_id = %ws, entry_id = %id, "cannot queue webhooks: {e:#}");
        }
    }
    audit::record(
        &state.pool,
        Some(ws),
        Some(current.user_id),
        action::ENTRY_SCHEDULED,
        &[id, schedule_id],
    )
    .await?;
    Ok(Json(entry_schedule(&state, ws, id).await?))
}

pub async fn delete_schedule(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    publisher(&state, &current, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let cancelled = scheduling::cancel(&mut tx, id, Reason::Cancelled, current.user_id)
        .await?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::CONFLICT,
                "no_schedule",
                "This entry has no scheduled publication.",
            )
        })?;
    let entry = webhooks::entry_ref(&mut tx, id).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    after_cancel(
        &state,
        ws,
        id,
        entry,
        Reason::Cancelled,
        current.user_id,
        cancelled.id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 予約を取り消した後: Webhook と監査ログ。訂正による取り消しは、決定どおり監査ログに分けて記録する（Q133）。
pub(super) async fn after_cancel(
    state: &AppState,
    ws: Uuid,
    id: Uuid,
    entry: Option<webhooks::EntryRef>,
    reason: Reason,
    actor: Uuid,
    schedule_id: Uuid,
) -> Result<(), ApiError> {
    if let Some(entry) = entry {
        let event = ScheduleEvent::Cancelled {
            reason: reason.as_str(),
        };
        if let Err(e) = webhooks::emit_schedule(&state.pool, &state.keys, ws, &entry, event).await {
            tracing::error!(workspace_id = %ws, entry_id = %id, "cannot queue webhooks: {e:#}");
        }
    }
    let act = if reason == Reason::Corrected {
        action::SCHEDULE_CANCELLED_BY_CORRECTION
    } else {
        action::SCHEDULE_CANCELLED
    };
    audit::record(&state.pool, Some(ws), Some(actor), act, &[id, schedule_id]).await?;
    Ok(())
}

/// 一覧の1件（予約と、記事の要約）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    schedule: ScheduleView,
    entry: Summary,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Lists {
    /// やり直している予約（公開できる人にだけ）。
    retrying: Vec<Item>,
    /// 自分の予約で、訂正による取り消しか失敗になったもの。
    attention: Vec<Item>,
}

pub async fn lists(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<Lists>, ApiError> {
    let ws = parse(&ws)?;
    let access = reader(&state, &current, ws).await?;
    let applied = readable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let retrying = if can_publish(&access) {
        scheduling::retrying(&mut tx).await?
    } else {
        Vec::new()
    };
    let attention = scheduling::needing_attention(&mut tx, current.user_id).await?;
    let summaries: HashMap<Uuid, Summary> = entries::list(&mut tx, &applied.manifest)
        .await?
        .into_iter()
        .map(|s| (s.id, s))
        .collect();
    tx.commit().await.map_err(anyhow::Error::from)?;
    let attach = |list: Vec<ScheduleView>| -> Vec<Item> {
        list.into_iter()
            .filter_map(|s| {
                summaries
                    .get(&s.schedule.entry_id)
                    .cloned()
                    .map(|entry| Item { schedule: s, entry })
            })
            .collect()
    };
    Ok(Json(Lists {
        retrying: attach(views(&state, ws, retrying).await?),
        attention: attach(views(&state, ws, attention).await?),
    }))
}

/// 予約の時刻に公開するジョブの中身。
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Job {
    workspace_id: Uuid,
    schedule_id: Uuid,
}

/// 予約の時刻に、予約版を公開する。公開できなければやり直し（Outcome::Retry）、24時間たったら失敗にする。
pub async fn run(state: &AppState, payload: &[u8], attempts: i32) -> anyhow::Result<Outcome> {
    let Ok(job) = serde_json::from_slice::<Job>(payload) else {
        return Ok(Outcome::Fail("invalid_payload"));
    };
    let ws = job.workspace_id;
    let (Some(applied), Some(db)) = (
        applied_schema(&state.pool, ws).await?,
        workspace_db::lookup(&state.pool, ws).await?,
    ) else {
        return Ok(Outcome::Done);
    };
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let Some((entry_id, schema_version, by, prepared, elapsed)) =
        scheduling::take_due(&mut tx, job.schedule_id).await?
    else {
        // 取り消し・予約し直し・公開済み
        return Ok(Outcome::Done);
    };
    let failure: &'static str = if applied.pending {
        "schema_migration_pending"
    } else {
        match publishing::publish_prepared(&mut tx, schema_version, entry_id, &prepared, by).await?
        {
            Ok(published) => {
                scheduling::mark_published(&mut tx, job.schedule_id).await?;
                let entry = webhooks::entry_ref(&mut tx, entry_id).await?;
                tx.commit().await?;
                let events = publish_events(&published);
                notify_webhooks(state, ws, &events, entry).await;
                schedule_hooks(state, ws, entry_id, events[0], Some(published.version_id)).await;
                return Ok(Outcome::Done);
            }
            Err(Refused::UrlTaken) => "url_taken",
            Err(Refused::NotFound) => "entry_not_found",
            Err(_) => "cannot_publish",
        }
    };
    if elapsed >= scheduling::GIVE_UP_AFTER_SECONDS {
        let failed = scheduling::mark_failed(&mut tx, job.schedule_id, failure).await?;
        let entry = webhooks::entry_ref(&mut tx, entry_id).await?;
        tx.commit().await?;
        if failed.is_some() {
            if let Some(entry) = entry {
                let event = ScheduleEvent::Cancelled { reason: "failed" };
                webhooks::emit_schedule(&state.pool, &state.keys, ws, &entry, event).await?;
            }
            audit::record(
                &state.pool,
                Some(ws),
                None,
                action::SCHEDULE_FAILED,
                &[entry_id, job.schedule_id],
            )
            .await?;
        }
        return Ok(Outcome::Done);
    }
    // 次にやり直す時刻は、ジョブのやり直しの間隔と同じ
    scheduling::mark_retry(
        &mut tx,
        job.schedule_id,
        failure,
        jobs::backoff_seconds(attempts),
    )
    .await?;
    tx.commit().await?;
    Ok(Outcome::Retry(failure))
}

/// その場の公開・訂正としての公開・削除で、開いている予約を取り消す（呼び出し側のトランザクションの中で）。
pub(super) async fn cancel_in(
    tx: &mut sqlx::PgConnection,
    id: Uuid,
    reason: Reason,
    current: &Current,
) -> anyhow::Result<Option<Schedule>> {
    scheduling::cancel(tx, id, reason, current.user_id).await
}
