//! 管理API: 埋め込みモデルの切り替え（08-search-and-related.md「モデルの変更」、Q30・Q138、M3）。
//!
//! | メソッド | パス | 内容 | 誰が |
//! |---|---|---|---|
//! | GET | /api/admin/embedding-switch | 切り替え先のモデルと、ワークスペースごとの状態・作るチャンクの数・トークン数（単価があれば費用）の見積もり | サーバー管理者 |
//! | POST | /api/admin/embedding-switch/start | 設定の切り替え先（`[llm.next_embedding]`）への切り替えを始める（止まっていれば再開する） | サーバー管理者（再認証） |
//!
//! - 切り替え先は設定ファイルにだけ書ける（この API では選べない。乗っ取られた管理者のログインで、送り先を変えられないように）。
//! - 開始は監査ログに残す（インスタンス全体の記録）。
//! - 埋め込みは、各ワークスペースの「埋め込みの作成」の予算の範囲で作る。

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;
use uuid::Uuid;

use super::AppState;
use super::error::ApiError;
use super::extract::AuthSession;
use crate::access;
use crate::audit::{self, action};
use crate::search::switch;
use crate::workspace_db;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceStatus {
    workspace_id: Uuid,
    name: String,
    state: &'static str,
    active_model: String,
    active_dimensions: u32,
    chunks_to_embed: i64,
    estimated_tokens: i64,
    estimated_cost: Option<f64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchView {
    /// LLMの接続先（`[llm]`）があるか。
    llm_configured: bool,
    /// 設定の今のモデルと、切り替え先のモデル（なければ null）。
    current_model: Option<String>,
    next_model: Option<String>,
    next_dimensions: Option<u32>,
    /// サーバー管理者が開始した切り替え先（設定の切り替え先と違えば、まだ始まっていない）。
    started_model: Option<String>,
    workspaces: Vec<WorkspaceStatus>,
}

async fn view(state: &AppState) -> Result<SwitchView, ApiError> {
    let llm = state.config.llm.as_ref();
    let next = llm.and_then(|l| l.next());
    let started = switch::started(&state.pool).await?;
    let mut workspaces = Vec::new();
    if llm.is_some() {
        let rows: Vec<(Uuid, String)> =
            sqlx::query_as("SELECT id, name FROM cms.workspaces ORDER BY created_at")
                .fetch_all(&state.pool)
                .await
                .map_err(anyhow::Error::from)?;
        for (id, name) in rows {
            let Some(db) = workspace_db::lookup(&state.pool, id).await? else {
                continue;
            };
            let Some(s) = switch::status(&state.pool, &state.config, &db, started.as_ref()).await?
            else {
                continue;
            };
            workspaces.push(WorkspaceStatus {
                workspace_id: id,
                name,
                state: s.state.name(),
                active_model: s.active.0,
                active_dimensions: s.active.1,
                chunks_to_embed: s.chunks_to_embed,
                estimated_tokens: s.estimated_tokens,
                estimated_cost: s.estimated_cost,
            });
        }
    }
    Ok(SwitchView {
        llm_configured: llm.is_some(),
        current_model: llm.map(|l| l.embedding_model.clone()),
        next_model: next.as_ref().map(|n| n.embedding_model.clone()),
        next_dimensions: next.as_ref().map(|n| n.embedding_dimensions),
        started_model: started.map(|(m, _)| m),
        workspaces,
    })
}

pub async fn get(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
) -> Result<Json<SwitchView>, ApiError> {
    access::server_admin(&current)?;
    Ok(Json(view(&state).await?))
}

pub async fn start(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
) -> Result<Json<SwitchView>, ApiError> {
    access::server_admin(&current)?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    let Some(next) = state.config.llm.as_ref().and_then(|l| l.next()) else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "no_next_embedding",
            "No embedding model to switch to is set in the config ([llm.next_embedding]).",
        ));
    };
    switch::start(
        &state.pool,
        &(next.embedding_model.clone(), next.embedding_dimensions),
        current.user_id,
    )
    .await?;
    audit::record(
        &state.pool,
        None,
        Some(current.user_id),
        action::EMBEDDING_SWITCH_STARTED,
        &[],
    )
    .await?;
    // すべてのワークスペースの検索用データのジョブを積む（止まっていたものも再開する）
    crate::search::index::schedule_all(&state.pool, &state.keys).await?;
    Ok(Json(view(&state).await?))
}
