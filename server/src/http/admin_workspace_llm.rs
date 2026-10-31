//! 管理API: ワークスペースの LLM の接続設定の上書き（10-llm-and-budget.md「接続設定の単位」、Q26、M4）。
//!
//! | メソッド | パス | 内容 | 誰が |
//! |---|---|---|---|
//! | GET | /api/admin/workspaces/{ws}/llm | 上書きを許されているか・選べる接続先の名前・今の上書き（キーは「設定済みか」だけ） | 重い権限 manageLlm を持つ人・サーバー管理者 |
//! | PUT | /api/admin/workspaces/{ws}/llm | 上書きを保存する（接続先は名前で選ぶ。URL は受け付けない） | 同上（再認証） |
//! | DELETE | /api/admin/workspaces/{ws}/llm | 上書きをやめて、インスタンスの設定に戻す（キーも消す） | 同上（再認証） |
//!
//! - 上書きを許すかどうかは、サーバー管理者が `PUT /api/admin/workspaces/{ws}/limits` の `llmOverride` で決める。
//! - 監査ログには、変えた項目の名前だけを残す。キーの値は、応答・ログ・監査ログ・エラーのどこにも出さない（INV-LLM-02）。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use super::AppState;
use super::admin_entries::parse;
use super::admin_publishing::holder;
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::access::Permission;
use crate::audit::{self, action};
use crate::workspace_llm::{self, Change, Rejected, View};

pub async fn get(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<View>, ApiError> {
    let ws = parse(&ws)?;
    holder(&state, &current, ws, Permission::ManageLlm).await?;
    Ok(Json(
        workspace_llm::view(&state.pool, &state.config, ws).await?,
    ))
}

pub async fn put(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    JsonBody(change): JsonBody<Change>,
) -> Result<Json<View>, ApiError> {
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
    Ok(Json(
        workspace_llm::view(&state.pool, &state.config, ws).await?,
    ))
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
