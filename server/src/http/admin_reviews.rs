//! 管理API: レビュー依頼（03-content-and-editing.md「レビュー（Q77）」、計画の8番）。
//!
//! | メソッド | パス | 内容 | 誰が |
//! |---|---|---|---|
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/review-request | 開いている依頼と、依頼の履歴 | 所属者・サーバー管理者 |
//! | POST | /api/admin/workspaces/{ws}/entries/{id}/review-request | 依頼する（`{reviewerId?}`） | 下書きを編集できる人 |
//! | PATCH | /api/admin/workspaces/{ws}/entries/{id}/review-request | 見てほしい人を変える（`{reviewerId}`。null で外す） | 依頼した本人・公開できる人 |
//! | POST | /api/admin/workspaces/{ws}/entries/{id}/review-request/reject | 差し戻す（`{comment}`） | 公開できる人 |
//! | POST | /api/admin/workspaces/{ws}/entries/{id}/review-request/withdraw | 取り下げる | 依頼した本人・公開できる人 |
//! | GET | /api/admin/workspaces/{ws}/reviewers | 見てほしい人の候補（公開できる人） | 下書きを編集できる人 |
//! | GET | /api/admin/workspaces/{ws}/review-requests | レビュー待ち（公開できる人に）と、自分の依頼で差し戻されたもの | 所属者・サーバー管理者 |
//!
//! - 公開できる人（管理者・編集者とサーバー管理者）は、依頼がなくても公開できる（レビューは必須ではない）。公開すると、
//!   開いている依頼は「公開済み」として閉じる（admin_publishing.rs）。
//! - 見てほしい人は、公開できる人（依頼した本人を除く）から選ぶ。目印だけで、他の公開できる人も差し戻し・公開できる。
//!   返すたびにいまの所属とロールと照合し、公開できなくなった人は「指定なし」（null）にする（原則3）。
//! - 「誰が」は、公開できる人と依頼した本人だけを表示名で見せる（未設定ならメールアドレス）。同じ名前が並ぶときは
//!   メールアドレスを添える。それ以外の人は null（他の所属者）にする。所属者の一覧の閲覧の権限（Q190）を広げないため。
//! - 監査ログには、記事と依頼のIDと操作の種類だけを残す（差し戻しのコメントは書かない。原則7）。

use std::collections::{HashMap, HashSet};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AppState;
use super::admin_entries::{db, editor, parse, readable, reader};
use super::admin_publishing::publisher;
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::access::{Access, Role};
use crate::audit::{self, action};
use crate::auth::session::Current;
use crate::content::entries::{self, Summary};
use crate::content::reviews::{self, Refused, Request, State as ReviewState};
use crate::workspace_db;

/// 公開できるか（管理者・編集者とサーバー管理者）。
fn can_publish(access: &Access) -> bool {
    access.server_admin || matches!(access.role, Some(Role::Admin | Role::Editor))
}

/// 所属者の、見せてよい名前の材料（表示名・メールアドレス・ロール）。
struct Who {
    display_name: Option<String>,
    email: String,
    role: Option<Role>,
}

async fn who(state: &AppState, ws: Uuid, ids: &[Uuid]) -> Result<HashMap<Uuid, Who>, ApiError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Uuid, Option<String>, String, Option<String>)> = sqlx::query_as(
        "SELECT u.id, u.display_name, u.email, m.role
         FROM cms.users u LEFT JOIN cms.memberships m ON m.user_id = u.id AND m.workspace_id = $1
         WHERE u.id = ANY($2)",
    )
    .bind(ws)
    .bind(ids)
    .fetch_all(&state.pool)
    .await
    .map_err(anyhow::Error::from)?;
    Ok(rows
        .into_iter()
        .map(|(id, display_name, email, role)| {
            (
                id,
                Who {
                    display_name,
                    email,
                    role: role.and_then(|r| r.parse().ok()),
                },
            )
        })
        .collect())
}

fn is_publisher(w: Option<&Who>) -> bool {
    w.is_some_and(|w| matches!(w.role, Some(Role::Admin | Role::Editor)))
}

/// 応答に出す人。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    user_id: Uuid,
    /// 表示名（未設定ならメールアドレス）。
    name: String,
    /// 同じ名前の人が並ぶときだけ添える。
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
}

/// 応答の中の人の名前を決める。見せてよい人（公開できる人と、依頼した本人）だけを入れる。
struct Names {
    people: HashMap<Uuid, Person>,
}

impl Names {
    fn build(who: &HashMap<Uuid, Who>, visible: &HashSet<Uuid>) -> Self {
        let mut people: HashMap<Uuid, Person> = visible
            .iter()
            .filter_map(|id| {
                who.get(id).map(|w| {
                    (
                        *id,
                        Person {
                            user_id: *id,
                            name: w.display_name.clone().unwrap_or_else(|| w.email.clone()),
                            email: None,
                        },
                    )
                })
            })
            .collect();
        let mut counts: HashMap<String, usize> = HashMap::new();
        for p in people.values() {
            *counts.entry(p.name.clone()).or_default() += 1;
        }
        for p in people.values_mut() {
            if counts.get(&p.name).copied().unwrap_or(0) > 1 {
                p.email = who.get(&p.user_id).map(|w| w.email.clone());
            }
        }
        Names { people }
    }

    fn get(&self, id: Option<Uuid>) -> Option<Person> {
        id.and_then(|id| self.people.get(&id).cloned())
    }
}

/// 応答の依頼。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestView {
    id: Uuid,
    entry_id: Uuid,
    state: ReviewState,
    requested_by: Option<Person>,
    /// 見てほしい人（指定なし、または公開できなくなった人なら null）。
    reviewer: Option<Person>,
    /// 自分が見てほしい人に指定されているか。
    assigned_to_me: bool,
    requested_revision: i64,
    /// 依頼の後に下書きが編集されたか（開いている依頼のときだけ）。
    changed_since_request: bool,
    comment: Option<String>,
    created_at: String,
    closed_at: Option<String>,
    closed_by: Option<Person>,
}

/// 依頼の一覧を、応答の形にする（人の名前は、見せてよい人だけ）。
async fn views(
    state: &AppState,
    ws: Uuid,
    current: &Current,
    list: Vec<Request>,
) -> Result<Vec<RequestView>, ApiError> {
    let ids: Vec<Uuid> = list
        .iter()
        .flat_map(|r| [Some(r.requested_by), r.reviewer, r.closed_by])
        .flatten()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let who = who(state, ws, &ids).await?;
    let mut visible = HashSet::new();
    for r in &list {
        // 依頼した本人（いまも所属している人）と、公開できる人だけ
        if who.get(&r.requested_by).is_some_and(|w| w.role.is_some()) {
            visible.insert(r.requested_by);
        }
        for id in [r.reviewer, r.closed_by].into_iter().flatten() {
            if is_publisher(who.get(&id)) {
                visible.insert(id);
            }
        }
    }
    let names = Names::build(&who, &visible);
    Ok(list
        .into_iter()
        .map(|r| {
            let reviewer = r.reviewer.filter(|id| is_publisher(who.get(id)));
            RequestView {
                id: r.id,
                entry_id: r.entry_id,
                state: r.state,
                requested_by: names.get(Some(r.requested_by)),
                reviewer: names.get(reviewer),
                assigned_to_me: reviewer == Some(current.user_id),
                requested_revision: r.requested_revision,
                changed_since_request: r.state == ReviewState::Open && r.changed_since_request,
                comment: r.comment,
                created_at: r.created_at,
                closed_at: r.closed_at,
                closed_by: names.get(r.closed_by),
            }
        })
        .collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryReview {
    open: Option<RequestView>,
    history: Vec<RequestView>,
}

async fn entry_review(
    state: &AppState,
    ws: Uuid,
    current: &Current,
    id: Uuid,
) -> Result<EntryReview, ApiError> {
    let db = db(state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let history = reviews::history(&mut tx, id).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    let mut history = views(state, ws, current, history).await?;
    let open = history
        .iter()
        .position(|r| r.state == ReviewState::Open)
        .map(|i| history.remove(i));
    Ok(EntryReview { open, history })
}

pub async fn get_review(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<EntryReview>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    reader(&state, &current, ws).await?;
    Ok(Json(entry_review(&state, ws, &current, id).await?))
}

/// 見てほしい人として指定できるか（公開できる所属者で、依頼した本人ではない）。
async fn check_reviewer(
    state: &AppState,
    ws: Uuid,
    reviewer: Option<Uuid>,
    requester: Uuid,
) -> Result<(), ApiError> {
    let Some(reviewer) = reviewer else {
        return Ok(());
    };
    let who = who(state, ws, &[reviewer]).await?;
    if reviewer == requester || !is_publisher(who.get(&reviewer)) {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_reviewer",
            "The reviewer must be an administrator or editor of this workspace other than the requester.",
        ));
    }
    Ok(())
}

fn no_open_review() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "no_open_review",
        "This entry has no open review request.",
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateReview {
    reviewer_id: Option<Uuid>,
}

pub async fn create_review(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
    JsonBody(req): JsonBody<CreateReview>,
) -> Result<(StatusCode, Json<EntryReview>), ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    editor(&state, &current, ws).await?;
    readable(&state, ws).await?;
    check_reviewer(&state, ws, req.reviewer_id, current.user_id).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let created = match reviews::create(&mut tx, id, current.user_id, req.reviewer_id).await? {
        Ok(r) => r,
        Err(Refused::NotFound) => return Err(ApiError::not_found()),
        Err(Refused::AlreadyOpen) => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "review_already_open",
                "This entry already has an open review request.",
            ));
        }
    };
    tx.commit().await.map_err(anyhow::Error::from)?;
    audit::record(
        &state.pool,
        Some(ws),
        Some(current.user_id),
        action::REVIEW_REQUESTED,
        &[id, created.id],
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(entry_review(&state, ws, &current, id).await?),
    ))
}

/// 依頼した本人か、公開できる人なら、開いている依頼を返す。
async fn requester_or_publisher(
    state: &AppState,
    current: &Current,
    ws: Uuid,
    id: Uuid,
) -> Result<Request, ApiError> {
    let access = reader(state, current, ws).await?;
    let db = db(state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let open = reviews::open(&mut tx, id).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    let open = open.ok_or_else(no_open_review)?;
    let requester = access.role.is_some() && open.requested_by == current.user_id;
    if !requester && !can_publish(&access) {
        return Err(crate::access::Denied::Forbidden.into());
    }
    Ok(open)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangeReviewer {
    reviewer_id: Option<Uuid>,
}

pub async fn change_reviewer(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
    JsonBody(req): JsonBody<ChangeReviewer>,
) -> Result<Json<EntryReview>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    let open = requester_or_publisher(&state, &current, ws, id).await?;
    check_reviewer(&state, ws, req.reviewer_id, open.requested_by).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let changed = reviews::set_reviewer(&mut tx, id, req.reviewer_id)
        .await?
        .ok_or_else(no_open_review)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    audit::record(
        &state.pool,
        Some(ws),
        Some(current.user_id),
        action::REVIEW_REVIEWER_CHANGED,
        &[id, changed.id],
    )
    .await?;
    Ok(Json(entry_review(&state, ws, &current, id).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reject {
    comment: String,
}

pub async fn reject(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
    JsonBody(req): JsonBody<Reject>,
) -> Result<Json<EntryReview>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    publisher(&state, &current, ws).await?;
    let comment = reviews::normalize_comment(&req.comment).ok_or_else(|| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_comment",
            format!(
                "comment: 1 to {} characters without control characters other than line breaks and tabs",
                reviews::MAX_COMMENT_CHARS
            ),
        )
    })?;
    close(
        &state,
        &current,
        ws,
        id,
        ReviewState::Rejected,
        Some(&comment),
        action::REVIEW_REJECTED,
    )
    .await
}

pub async fn withdraw(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<EntryReview>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    requester_or_publisher(&state, &current, ws, id).await?;
    close(
        &state,
        &current,
        ws,
        id,
        ReviewState::Withdrawn,
        None,
        action::REVIEW_WITHDRAWN,
    )
    .await
}

async fn close(
    state: &AppState,
    current: &Current,
    ws: Uuid,
    id: Uuid,
    to: ReviewState,
    comment: Option<&str>,
    act: &str,
) -> Result<Json<EntryReview>, ApiError> {
    let db = db(state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let closed = reviews::close(&mut tx, id, to, current.user_id, comment)
        .await?
        .ok_or_else(no_open_review)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    audit::record(
        &state.pool,
        Some(ws),
        Some(current.user_id),
        act,
        &[id, closed.id],
    )
    .await?;
    Ok(Json(entry_review(state, ws, current, id).await?))
}

pub async fn reviewers(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<Vec<Person>>, ApiError> {
    let ws = parse(&ws)?;
    editor(&state, &current, ws).await?;
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM cms.memberships WHERE workspace_id = $1 AND role IN ('admin', 'editor')
         ORDER BY created_at, user_id",
    )
    .bind(ws)
    .fetch_all(&state.pool)
    .await
    .map_err(anyhow::Error::from)?;
    let who = who(&state, ws, &ids).await?;
    let names = Names::build(&who, &ids.iter().copied().collect());
    Ok(Json(
        ids.into_iter()
            .filter_map(|id| names.get(Some(id)))
            .collect(),
    ))
}

/// 一覧の1件（依頼と、記事の要約）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    request: RequestView,
    entry: Summary,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Lists {
    /// レビュー待ち（公開できる人にだけ。自分が指定されたものが先）。
    waiting: Vec<Item>,
    /// 自分の依頼で、差し戻されたままのもの。
    rejected: Vec<Item>,
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
    let waiting = if can_publish(&access) {
        reviews::waiting(&mut tx).await?
    } else {
        Vec::new()
    };
    let rejected = reviews::rejected_for(&mut tx, current.user_id).await?;
    let summaries: HashMap<Uuid, Summary> = entries::list(&mut tx, &applied.manifest)
        .await?
        .into_iter()
        .map(|s| (s.id, s))
        .collect();
    tx.commit().await.map_err(anyhow::Error::from)?;
    let attach = |views: Vec<RequestView>| -> Vec<Item> {
        views
            .into_iter()
            .filter_map(|r| {
                summaries
                    .get(&r.entry_id)
                    .cloned()
                    .map(|entry| Item { request: r, entry })
            })
            .collect()
    };
    let mut waiting = attach(views(&state, ws, &current, waiting).await?);
    // 自分が指定されたものを先に（それぞれの中では古い順のまま）
    waiting.sort_by_key(|i| !i.request.assigned_to_me);
    let rejected = attach(views(&state, ws, &current, rejected).await?);
    Ok(Json(Lists { waiting, rejected }))
}
