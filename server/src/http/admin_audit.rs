//! 管理API: 監査ログの閲覧（11-permissions-and-audit.md「監査ログ」、Q80, Q81, Q122）。読むだけで、編集・削除の操作はない（INV-AUD-02）。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | GET | /api/admin/workspaces/{ws}/audit-log | ワークスペースの記録（そのワークスペースの管理者とサーバー管理者） |
//! | GET | /api/admin/audit-log | インスタンス全体の記録（サーバー管理者だけ） |
//! | GET | /api/admin/workspaces/{ws}/audit-log/export | ワークスペースの記録のダウンロード（1行1件の JSON。M2） |
//! | GET | /api/admin/audit-log/export | インスタンス全体の記録のダウンロード（サーバー管理者だけ。M2） |
//!
//! - 新しい順。`?limit=`（1〜500、既定100）と `?before=<記録のID>`（その記録より古いもの）で読み進める。
//!   続きがあれば `nextBefore` に次に渡すIDが入る。
//! - 絞り込み（M2、計画の14番）: `?action=<操作の種類>`、`?actor=<利用者のID>`、`?from=`・`?to=`（RFC 3339。from は含み、to は含まない）。
//! - ダウンロードは、絞り込んだ結果を新しい順に最大1万件。1行は外部への書き出しと同じ形
//!   （`{"type":"audit","id","at","workspaceId","actorUserId","action","targetIds"}`）。
//! - 記録には操作の種類・操作した人・対象のID・時刻だけがある（内容は記録していない。INV-AUD-01）。

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use uuid::Uuid;

use super::AppState;
use super::error::ApiError;
use super::extract::AuthSession;
use crate::access;
use crate::audit::{self, Entry, Filter, Scope};
use crate::timefmt;

const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 500;
/// ダウンロードの件数の上限。
const EXPORT_LIMIT: i64 = 10_000;
/// 操作の種類の長さの上限（記録する名前は短い）。
const MAX_ACTION_CHARS: usize = 100;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    entries: Vec<Entry>,
    /// 続きを読むときに `before` に渡すID。続きがなければ null。
    next_before: Option<Uuid>,
}

/// 受け付けるクエリの名前。
const PARAMS: [&str; 6] = ["limit", "before", "action", "actor", "from", "to"];

fn filter_params(q: &HashMap<String, String>) -> Result<Filter, ApiError> {
    if q.keys().any(|k| !PARAMS.contains(&k.as_str())) {
        return Err(ApiError::bad_request(
            "Only `limit`, `before`, `action`, `actor`, `from` and `to` are accepted.",
        ));
    }
    let action = q.get("action").cloned();
    if action
        .as_deref()
        .is_some_and(|a| a.is_empty() || a.len() > MAX_ACTION_CHARS)
    {
        return Err(ApiError::bad_request(
            "`action` is not a valid action name.",
        ));
    }
    let actor_user_id = match q.get("actor") {
        None => None,
        Some(v) => Some(
            Uuid::parse_str(v).map_err(|_| ApiError::bad_request("`actor` must be a user ID."))?,
        ),
    };
    let time = |name: &str| -> Result<Option<String>, ApiError> {
        match q.get(name) {
            None => Ok(None),
            Some(v) if timefmt::is_rfc3339(v) => Ok(Some(v.clone())),
            Some(_) => Err(ApiError::bad_request(format!(
                "`{name}` must be a date and time in RFC 3339."
            ))),
        }
    };
    Ok(Filter {
        action,
        actor_user_id,
        from: time("from")?,
        to: time("to")?,
    })
}

fn page_params(q: &HashMap<String, String>) -> Result<(Option<Uuid>, i64), ApiError> {
    let limit = match q.get("limit") {
        None => DEFAULT_LIMIT,
        Some(v) => v
            .parse::<i64>()
            .ok()
            .filter(|n| (1..=MAX_LIMIT).contains(n))
            .ok_or_else(|| ApiError::bad_request(format!("`limit` must be 1 to {MAX_LIMIT}.")))?,
    };
    let before = match q.get("before") {
        None => None,
        Some(v) => Some(
            Uuid::parse_str(v)
                .map_err(|_| ApiError::bad_request("`before` must be an audit log entry ID."))?,
        ),
    };
    Ok((before, limit))
}

async fn read(
    state: &AppState,
    scope: Scope,
    q: &HashMap<String, String>,
) -> Result<Json<Page>, ApiError> {
    let filter = filter_params(q)?;
    let (before, limit) = page_params(q)?;
    // 1件多く読み、続きがあるかを知る
    let mut entries = audit::list(&state.pool, scope, &filter, before, limit + 1).await?;
    let more = entries.len() as i64 > limit;
    entries.truncate(limit as usize);
    let next_before = if more {
        entries.last().map(|e| e.id)
    } else {
        None
    };
    Ok(Json(Page {
        entries,
        next_before,
    }))
}

pub async fn workspace(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Json<Page>, ApiError> {
    let ws = Uuid::parse_str(&ws).map_err(|_| ApiError::not_found())?;
    access::workspace_admin(&state.pool, &current, ws).await??;
    read(&state, Scope::Workspace(ws), &q).await
}

pub async fn instance(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Json<Page>, ApiError> {
    access::server_admin(&current)?;
    read(&state, Scope::Instance, &q).await
}

/// ダウンロード（1行1件の JSON。絞り込みは閲覧と同じ。`limit` と `before` は使わない）。
async fn export(
    state: &AppState,
    scope: Scope,
    q: &HashMap<String, String>,
) -> Result<Response, ApiError> {
    if q.contains_key("limit") || q.contains_key("before") {
        return Err(ApiError::bad_request(
            "The download takes only `action`, `actor`, `from` and `to`.",
        ));
    }
    let filter = filter_params(q)?;
    let entries = audit::list(&state.pool, scope, &filter, None, EXPORT_LIMIT).await?;
    let mut body = String::new();
    for e in &entries {
        body.push_str(&serde_json::to_string(&audit::line(e)).map_err(anyhow::Error::from)?);
        body.push('\n');
    }
    let mut res = (StatusCode::OK, body).into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-ndjson"),
    );
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"audit-log.jsonl\""),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(res)
}

pub async fn workspace_export(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let ws = Uuid::parse_str(&ws).map_err(|_| ApiError::not_found())?;
    access::workspace_admin(&state.pool, &current, ws).await??;
    export(&state, Scope::Workspace(ws), &q).await
}

pub async fn instance_export(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    access::server_admin(&current)?;
    export(&state, Scope::Instance, &q).await
}
