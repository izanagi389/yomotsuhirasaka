//! 管理API: ワークスペースの LLM の接続設定の上書き（10-llm-and-budget.md「接続設定の単位」、Q26、M4）。
//!
//! | メソッド | パス | 内容 | 誰が |
//! |---|---|---|---|
//! | GET | /api/admin/workspaces/{ws}/llm | 上書きを許されているか・選べる接続先の名前・今の上書き（キーは「設定済みか」だけ） | 重い権限 manageLlm を持つ人・サーバー管理者 |
//! | PUT | /api/admin/workspaces/{ws}/llm | 上書きを保存する（接続先は名前で選ぶ。URL は受け付けない） | 同上（再認証） |
//! | DELETE | /api/admin/workspaces/{ws}/llm | 上書きをやめて、インスタンスの設定に戻す（キーも消す） | 同上（再認証） |
//! | POST | /api/admin/workspaces/{ws}/llm/embedding-switch | 埋め込みのモデルの切り替えを開始する（見積もりを見てから） | 同上（再認証） |
//!
//! - GET は、埋め込みのモデルの切り替えの状態と見積もり（`embedding`）も返す。モデルを変えたら、すべてのチャンクの埋め込みを
//!   作り直すので、開始の操作を待つ。そろうまで前のモデルで検索を続ける（`search::switch`）。
//!
//! - 上書きを許すかどうかは、サーバー管理者が `PUT /api/admin/workspaces/{ws}/limits` の `llmOverride` で決める。
//! - 監査ログには、変えた項目の名前だけを残す。キーの値は、応答・ログ・監査ログ・エラーのどこにも出さない（INV-LLM-02）。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Serialize;

use super::AppState;
use super::admin_entries::parse;
use super::admin_publishing::holder;
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::access::Permission;
use crate::audit::{self, action};
use crate::search::switch;
use crate::workspace_db;
use crate::workspace_llm::{self, Change, Rejected, View};

/// 埋め込みのモデルの切り替えの状態と見積もり。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingView {
    /// notStarted・inProgress・paused・switched・needsMigration（`switch::State`）。
    pub state: &'static str,
    pub active_model: String,
    pub desired_model: String,
    pub chunks_to_embed: i64,
    pub estimated_tokens: i64,
    pub estimated_cost: Option<f64>,
    /// 切り替えを止めた理由（次元数が合わないなど）。
    pub failed_code: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmView {
    #[serde(flatten)]
    pub view: View,
    /// LLM の設定がなければ null。
    pub embedding: Option<EmbeddingView>,
}

async fn llm_view(state: &AppState, ws: uuid::Uuid) -> Result<LlmView, ApiError> {
    let view = workspace_llm::view(&state.pool, &state.config, ws).await?;
    let setup = workspace_llm::embedding(&state.pool, &state.keys, &state.config, ws).await?;
    let db = workspace_db::lookup(&state.pool, ws)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let embedding = match setup {
        Some(setup) => {
            let s = switch::status(&state.pool, &setup.llm, &db, &setup.started).await?;
            Some(EmbeddingView {
                state: s.state.name(),
                active_model: s.active.0,
                desired_model: s.desired.0,
                chunks_to_embed: s.chunks_to_embed,
                estimated_tokens: s.estimated_tokens,
                estimated_cost: s.estimated_cost,
                failed_code: workspace_llm::embedding_switch_failure(&state.pool, ws).await?,
            })
        }
        None => None,
    };
    Ok(LlmView { view, embedding })
}

pub async fn get(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<LlmView>, ApiError> {
    let ws = parse(&ws)?;
    holder(&state, &current, ws, Permission::ManageLlm).await?;
    Ok(Json(llm_view(&state, ws).await?))
}

pub async fn put(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    JsonBody(change): JsonBody<Change>,
) -> Result<Json<LlmView>, ApiError> {
    let ws = parse(&ws)?;
    holder(&state, &current, ws, Permission::ManageLlm).await?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    match workspace_llm::save(
        &state.pool,
        &state.keys,
        &state.config,
        ws,
        current.user_id,
        &change,
    )
    .await?
    {
        Ok(()) => {}
        Err(Rejected::NotAllowed) => {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "llm_override_not_allowed",
                "The server administrator does not allow this workspace to override the LLM settings.",
            ));
        }
        Err(Rejected::UnknownEmbeddingModel) => {
            return Err(ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "unknown_embedding_model",
                "Choose one of the embedding models the server administrator allows.",
            ));
        }
        Err(Rejected::UnknownEndpoint) => {
            return Err(ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "unknown_llm_endpoint",
                "Choose one of the endpoints the server administrator allows.",
            ));
        }
        Err(Rejected::Invalid(problem)) => {
            return Err(ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_llm_settings",
                format!("The LLM settings are not valid: {problem}"),
            ));
        }
    }
    audit::record_full(
        &state.pool,
        Some(ws),
        Some(current.user_id),
        action::LLM_OVERRIDE_CHANGED,
        &[],
        false,
        Some(&serde_json::json!({ "fields": change.fields() })),
    )
    .await?;
    Ok(Json(llm_view(&state, ws).await?))
}

pub async fn delete(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<StatusCode, ApiError> {
    let ws = parse(&ws)?;
    holder(&state, &current, ws, Permission::ManageLlm).await?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    if workspace_llm::remove(&state.pool, ws).await? {
        audit::record(
            &state.pool,
            Some(ws),
            Some(current.user_id),
            action::LLM_OVERRIDE_REMOVED,
            &[],
        )
        .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// 埋め込みのモデルの切り替えを開始する（今の見積もりの切り替え先へ）。切り替えるものがなければ 409。
pub async fn start_embedding_switch(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<LlmView>, ApiError> {
    let ws = parse(&ws)?;
    holder(&state, &current, ws, Permission::ManageLlm).await?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    let view = llm_view(&state, ws).await?;
    let Some(e) = &view.embedding else {
        return Err(nothing_to_switch());
    };
    if !matches!(e.state, "notStarted" | "needsMigration") && e.failed_code.is_none() {
        return Err(nothing_to_switch());
    }
    let setup = workspace_llm::embedding(&state.pool, &state.keys, &state.config, ws)
        .await?
        .ok_or_else(nothing_to_switch)?;
    let db = workspace_db::lookup(&state.pool, ws)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let s = switch::status(&state.pool, &setup.llm, &db, &setup.started).await?;
    if s.desired == s.active {
        return Err(nothing_to_switch());
    }
    // ワークスペースから始められるのは、ワークスペースが選んだモデルへの切り替えと、インスタンスの今のモデルへ戻す切り替えだけ。
    // インスタンスの切り替え先（[llm.next_embedding]）への切り替えは、サーバー管理者が始める（/api/admin/embedding-switch）
    if !workspace_llm::workspace_may_start(&setup, &s.desired) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "embedding_switch_server_admin",
            "Only a server administrator can start the switch to the server's next embedding model.",
        ));
    }
    workspace_llm::start_embedding_switch(&state.pool, ws, &s.desired, current.user_id).await?;
    audit::record(
        &state.pool,
        Some(ws),
        Some(current.user_id),
        action::EMBEDDING_SWITCH_STARTED,
        &[ws],
    )
    .await?;
    // 新しいモデルの埋め込みを、すぐに作り始める
    super::admin_publishing::schedule_search(&state, ws).await;
    Ok(Json(llm_view(&state, ws).await?))
}

fn nothing_to_switch() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "no_embedding_switch",
        "There is no embedding model to switch to.",
    )
}
