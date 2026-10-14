//! 管理API: 監査ログの閲覧（11-permissions-and-audit.md「監査ログ」、Q80, Q81, Q122）。読むだけで、編集・削除の操作はない（INV-AUD-02）。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | GET | /api/admin/workspaces/{ws}/audit-log | ワークスペースの記録（そのワークスペースの管理者とサーバー管理者） |
//! | GET | /api/admin/audit-log | インスタンス全体の記録（サーバー管理者だけ） |
//!
//! - 新しい順。`?limit=`（1〜500、既定100）と `?before=<記録のID>`（その記録より古いもの）で読み進める。
//!   続きがあれば `nextBefore` に次に渡すIDが入る。
//! - 記録には操作の種類・操作した人・対象のID・時刻だけがある（内容は記録していない。INV-AUD-01）。

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Serialize;
use uuid::Uuid;

use super::AppState;
use super::error::ApiError;
use super::extract::AuthSession;
use crate::access;
use crate::audit::{self, Entry, Scope};

const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 500;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    entries: Vec<Entry>,
    /// 続きを読むときに `before` に渡すID。続きがなければ null。
    next_before: Option<Uuid>,
}

fn page_params(q: &HashMap<String, String>) -> Result<(Option<Uuid>, i64), ApiError> {
    if q.keys().any(|k| k != "limit" && k != "before") {
        return Err(ApiError::bad_request(
            "Only `limit` and `before` are accepted.",
        ));
    }
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
    let (before, limit) = page_params(q)?;
    // 1件多く読み、続きがあるかを知る
    let mut entries = audit::list(&state.pool, scope, before, limit + 1).await?;
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
