//! 管理API: 編集のロック（ソフトロック。03-content-and-editing.md「同時編集（Q73）」、計画の10番）と、引き継ぎの設定。
//!
//! | メソッド | パス | 内容 | 誰が |
//! |---|---|---|---|
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/edit-lock | 編集中の人（`?mine=<lockId>` で自分のロックか） | 所属者・サーバー管理者 |
//! | POST | /api/admin/workspaces/{ws}/entries/{id}/edit-lock | ロックを取る（記事の画面を開いたとき） | 下書きを編集できる人 |
//! | POST | /api/admin/workspaces/{ws}/entries/{id}/edit-lock/take-over | 編集を引き継ぐ | 下書きを編集できる人（設定で「権限を持つ人だけ」にできる） |
//! | POST | /api/admin/workspaces/{ws}/entries/{id}/edit-lock/{lockId}/heartbeat | 生存の知らせ | 下書きを編集できる人 |
//! | DELETE | /api/admin/workspaces/{ws}/entries/{id}/edit-lock/{lockId} | ロックを外す | 下書きを編集できる人 |
//! | PATCH | /api/admin/workspaces/{ws}/settings | ワークスペースの設定（`{takeOverPolicy}`） | 管理者・サーバー管理者（多要素認証・再認証） |
//!
//! - ロックのIDは保存の権利なので、取った本人（その画面）にだけ返す。ほかの人には、編集中の人と時刻だけを返す。
//! - 編集中の人は表示名で見せる（記事を開ける所属者全員に。表示名がなければ null = 「他の所属者」）。
//! - 引き継ぎは監査ログに残す（同じ人の別の画面からの引き継ぎは残さない）。ロックが変わるたびに、即時の知らせを出す。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AppState;
use super::admin_entries::{db, editor, parse, reader};
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use super::realtime::{self, Event};
use crate::access::{self, Permission};
use crate::audit::{self, action};
use crate::auth::session::Current;
use crate::content::locks::{self, Lock, Refused};
use crate::workspace_db;

/// 下書きの保存を、ほかの画面が編集中のために断る（INV-LOCK-01）。
pub(super) fn entry_locked() -> ApiError {
    ApiError::new(
        StatusCode::LOCKED,
        "entry_locked",
        "Someone else is editing this entry. Take over editing to save.",
    )
}

/// 編集中の人。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Holder {
    user_id: Uuid,
    /// 表示名（未設定なら null。画面は「他の所属者」と出す）。
    name: Option<String>,
    /// 自分か（別の画面で開いている自分）。
    same_user: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LockState {
    /// 編集中の人（有効なロックがなければ null）。
    holder: Option<Holder>,
    /// `?mine=` で渡したロックが、いまも有効なロックか。
    is_mine: bool,
    acquired_at: Option<String>,
    expires_at: Option<String>,
}

async fn holder(state: &AppState, current: &Current, lock: &Lock) -> Result<Holder, ApiError> {
    let name: Option<Option<String>> =
        sqlx::query_scalar("SELECT display_name FROM cms.users WHERE id = $1")
            .bind(lock.user_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(anyhow::Error::from)?;
    Ok(Holder {
        user_id: lock.user_id,
        name: name.flatten(),
        same_user: lock.user_id == current.user_id,
    })
}

async fn lock_state(
    state: &AppState,
    current: &Current,
    lock: Option<Lock>,
    mine: Option<Uuid>,
) -> Result<LockState, ApiError> {
    Ok(match lock {
        Some(l) => LockState {
            holder: Some(holder(state, current, &l).await?),
            is_mine: mine == Some(l.lock_id),
            acquired_at: Some(l.acquired_at),
            expires_at: Some(l.expires_at),
        },
        None => LockState {
            holder: None,
            is_mine: false,
            acquired_at: None,
            expires_at: None,
        },
    })
}

#[derive(Deserialize)]
pub struct MineQuery {
    mine: Option<Uuid>,
}

pub async fn get_lock(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
    Query(q): Query<MineQuery>,
) -> Result<Json<LockState>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    reader(&state, &current, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let lock = locks::current(&mut tx, id).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(lock_state(&state, &current, lock, q.mine).await?))
}

/// ロックを取ったとき・引き継いだときの応答（ロックのIDは本人にだけ）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Acquired {
    lock_id: Uuid,
    expires_at: String,
    /// 生存を知らせる間隔（秒）。
    heartbeat_seconds: f64,
}

fn acquired(lock: &Lock) -> Acquired {
    Acquired {
        lock_id: lock.lock_id,
        expires_at: lock.expires_at.clone(),
        heartbeat_seconds: locks::LEASE_SECONDS / 3.0,
    }
}

async fn changed(state: &AppState, ws: Uuid, id: Uuid) {
    realtime::publish(
        &state.pool,
        Event {
            kind: "editLock".into(),
            workspace_id: ws,
            entry_id: id,
        },
    )
    .await;
}

pub async fn acquire(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<Acquired>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    editor(&state, &current, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let lock = match locks::acquire(&mut tx, id, current.user_id, current.id).await? {
        Ok(l) => l,
        Err(Refused::NotFound) => return Err(ApiError::not_found()),
        Err(Refused::Held(_)) => return Err(entry_locked()),
    };
    tx.commit().await.map_err(anyhow::Error::from)?;
    changed(&state, ws, id).await;
    Ok(Json(acquired(&lock)))
}

/// 引き継ぎの設定（行がなければ既定の anyone）。
pub(super) async fn take_over_policy_of(pool: &sqlx::PgPool, ws: Uuid) -> Result<String, ApiError> {
    let policy: Option<String> = sqlx::query_scalar(
        "SELECT take_over_policy FROM cms.workspace_settings WHERE workspace_id = $1",
    )
    .bind(ws)
    .fetch_optional(pool)
    .await
    .map_err(anyhow::Error::from)?;
    Ok(policy.unwrap_or_else(|| "anyone".to_owned()))
}

pub async fn take_over(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<Acquired>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    let access = editor(&state, &current, ws).await?;
    let restricted = take_over_policy_of(&state.pool, ws).await? == "permission";
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    // 権限が要る設定でも、自分の別の画面からは引き継げる
    if restricted && !access.has(Permission::TakeOverEditing) {
        let held = locks::current(&mut tx, id).await?;
        if held.is_some_and(|l| l.user_id != current.user_id) {
            return Err(access::Denied::Forbidden.into());
        }
    }
    let (lock, previous) = locks::take_over(&mut tx, id, current.user_id, current.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    if let Some(previous) = previous.filter(|p| p.user_id != current.user_id) {
        audit::record(
            &state.pool,
            Some(ws),
            Some(current.user_id),
            action::EDIT_TAKEN_OVER,
            &[id, previous.user_id],
        )
        .await?;
    }
    changed(&state, ws, id).await;
    Ok(Json(acquired(&lock)))
}

pub async fn heartbeat(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id, lock_id)): Path<(String, String, String)>,
) -> Result<Json<Acquired>, ApiError> {
    let (ws, id, lock_id) = (parse(&ws)?, parse(&id)?, parse(&lock_id)?);
    editor(&state, &current, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let lock = locks::heartbeat(&mut tx, id, lock_id).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    let lock = lock.ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "lock_lost",
            "The edit lock was taken over or expired.",
        )
    })?;
    Ok(Json(acquired(&lock)))
}

pub async fn release(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id, lock_id)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let (ws, id, lock_id) = (parse(&ws)?, parse(&id)?, parse(&lock_id)?);
    editor(&state, &current, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let released = locks::release(&mut tx, id, lock_id).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    if released {
        changed(&state, ws, id).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    /// 編集の引き継ぎ: `anyone`（下書きを編集できる人なら誰でも）か `permission`（重い権限を持つ人だけ）。
    take_over_policy: String,
}

/// ワークスペースの設定の変更（管理者・多要素認証・再認証。監査ログに記録）。
pub async fn patch_settings(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    JsonBody(req): JsonBody<Settings>,
) -> Result<StatusCode, ApiError> {
    let ws = parse(&ws)?;
    access::workspace_admin(&state.pool, &current, ws).await??;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    if !matches!(req.take_over_policy.as_str(), "anyone" | "permission") {
        return Err(ApiError::bad_request(
            "`takeOverPolicy` must be \"anyone\" or \"permission\".",
        ));
    }
    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    sqlx::query(
        "INSERT INTO cms.workspace_settings (workspace_id, take_over_policy) VALUES ($1, $2)
         ON CONFLICT (workspace_id) DO UPDATE SET take_over_policy = EXCLUDED.take_over_policy",
    )
    .bind(ws)
    .bind(&req.take_over_policy)
    .execute(&mut *tx)
    .await
    .map_err(anyhow::Error::from)?;
    audit::record(
        &mut *tx,
        Some(ws),
        Some(current.user_id),
        action::WORKSPACE_SETTINGS_CHANGED,
        &[ws],
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}
