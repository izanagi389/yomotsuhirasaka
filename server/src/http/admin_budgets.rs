//! 管理API: LLMの予算（10-llm-and-budget.md「予算」、Q70）。M3は「埋め込みの作成」だけ。
//!
//! | メソッド | パス | 内容 | 誰が |
//! |---|---|---|---|
//! | GET | /api/admin/workspaces/{ws}/budgets/embedding | 上限（ワークスペース・天井・効く上限）と、今月・今日の使用量 | 重い権限 manageLlm を持つ人・サーバー管理者 |
//! | PUT | /api/admin/workspaces/{ws}/budgets/embedding | ワークスペースの上限を変える（天井の範囲で。null は上限なし） | 同上（再認証） |
//!
//! - 天井はサーバー管理者が `PUT /api/admin/workspaces/{ws}/limits` で決める。
//! - 変更は監査ログに残す（値は書かない）。
//! - 単価を設定していれば（`[llm] embedding_price_per_million_tokens`）、今月の使用量の費用も返す（Q138）。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AppState;
use super::admin_entries::parse;
use super::admin_publishing::holder;
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::access::Permission;
use crate::audit::{self, action};
use crate::budget::{self, Kind};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetView {
    /// LLMの接続先（`[llm]`）があるか。なければ埋め込みは作らない。
    llm_configured: bool,
    /// ワークスペースが決めた上限（null は上限なし＝天井）。
    tokens_per_month: Option<i64>,
    tokens_per_day: Option<i64>,
    /// サーバー管理者の天井。
    ceiling_tokens_per_month: i64,
    ceiling_tokens_per_day: i64,
    /// 効く上限（ワークスペースの上限と天井の小さいほう）。
    effective_tokens_per_month: i64,
    effective_tokens_per_day: i64,
    /// 今月・今日の使用量（UTC）。
    used_this_month: i64,
    used_today: i64,
    /// 今の期間に、予算が足りずに初めて断った時刻（上限を変えるまで）。
    exhausted_at: Option<String>,
    /// 単価（100万トークンあたり）と、今月の費用（単価があるときだけ）。
    price_per_million_tokens: Option<f64>,
    cost_this_month: Option<f64>,
}

async fn view(state: &AppState, ws: Uuid) -> Result<BudgetView, ApiError> {
    let limits = budget::limits(&state.pool, &state.config, ws, Kind::Embedding).await?;
    let usage = budget::usage(&state.pool, ws, Kind::Embedding).await?;
    let effective = limits.effective();
    let price = state
        .config
        .llm
        .as_ref()
        .and_then(|l| l.embedding_price_per_million_tokens);
    Ok(BudgetView {
        llm_configured: state.config.llm.is_some(),
        tokens_per_month: limits.month,
        tokens_per_day: limits.day,
        ceiling_tokens_per_month: limits.ceiling.per_month,
        ceiling_tokens_per_day: limits.ceiling.per_day,
        effective_tokens_per_month: effective.per_month,
        effective_tokens_per_day: effective.per_day,
        used_this_month: usage.month,
        used_today: usage.day,
        exhausted_at: usage.exhausted_at,
        price_per_million_tokens: price,
        cost_this_month: price.map(|p| p * usage.month as f64 / 1_000_000.0),
    })
}

pub async fn get_embedding(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<BudgetView>, ApiError> {
    let ws = parse(&ws)?;
    holder(&state, &current, ws, Permission::ManageLlm).await?;
    Ok(Json(view(&state, ws).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetBudget {
    /// 上限（トークン数）。null は上限なし（天井がそのまま効く）。
    tokens_per_month: Option<i64>,
    tokens_per_day: Option<i64>,
}

pub async fn set_embedding(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    JsonBody(req): JsonBody<SetBudget>,
) -> Result<Json<BudgetView>, ApiError> {
    let ws = parse(&ws)?;
    holder(&state, &current, ws, Permission::ManageLlm).await?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    if [req.tokens_per_month, req.tokens_per_day]
        .iter()
        .any(|v| v.is_some_and(|t| t < 0))
    {
        return Err(ApiError::bad_request(
            "`tokensPerMonth` and `tokensPerDay` must not be negative.",
        ));
    }
    // 天井の範囲でだけ決められる（天井は使う瞬間にも照合する）
    let ceiling = budget::ceiling(&state.pool, &state.config, ws).await?;
    if req.tokens_per_month.is_some_and(|t| t > ceiling.per_month)
        || req.tokens_per_day.is_some_and(|t| t > ceiling.per_day)
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "budget_exceeds_ceiling",
            "The budget exceeds the limit set by the server administrator.",
        ));
    }
    budget::set_limits(
        &state.pool,
        ws,
        Kind::Embedding,
        req.tokens_per_month,
        req.tokens_per_day,
        current.user_id,
    )
    .await?;
    audit::record(
        &state.pool,
        Some(ws),
        Some(current.user_id),
        action::BUDGET_CHANGED,
        &[ws],
    )
    .await?;
    // 予算で止まっていた検索用データの作成を、すぐにやり直す
    super::admin_publishing::schedule_search(&state, ws).await;
    Ok(Json(view(&state, ws).await?))
}
