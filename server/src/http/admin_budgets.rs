//! 管理API: LLMの予算（10-llm-and-budget.md「予算」、Q70）。「埋め込みの作成」と「訪問者向けの回答」（M4。入力・出力別）。
//!
//! | メソッド | パス | 内容 | 誰が |
//! |---|---|---|---|
//! | GET | /api/admin/workspaces/{ws}/budgets/embedding | 上限（ワークスペース・天井・効く上限）と、今月・今日の使用量 | 重い権限 manageLlm を持つ人・サーバー管理者 |
//! | PUT | /api/admin/workspaces/{ws}/budgets/embedding | ワークスペースの上限を変える（天井の範囲で。null は上限なし） | 同上（再認証） |
//! | GET | /api/admin/workspaces/{ws}/budgets/answer | 訪問者向けの回答の予算（入力・出力のそれぞれに、埋め込みと同じ項目） | 同上 |
//! | PUT | /api/admin/workspaces/{ws}/budgets/answer | 同じく、ワークスペースの上限を変える（`input`・`output`。項目がなければ変えない） | 同上（再認証） |
//!
//! - 天井はサーバー管理者が `PUT /api/admin/workspaces/{ws}/limits` で決める。
//! - 変更は監査ログに残す（値は書かない）。
//! - 単価を設定していれば（`[llm] embedding_price_per_million_tokens`、回答は `chat_input/output_price_per_million_tokens`）、
//!   今月の使用量の費用も返す（Q138）。

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
use crate::budget::{self, Direction, Kind};

/// 1つの予算（種類と向き）の上限と使用量。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartView {
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetView {
    /// LLMの接続先（`[llm]`）があるか。なければ埋め込みは作らない。
    llm_configured: bool,
    #[serde(flatten)]
    part: PartView,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerBudgetView {
    /// 回答の生成の設定（`[llm] chat_model`）があるか。なければ回答しない。
    chat_configured: bool,
    input: PartView,
    output: PartView,
}

async fn part(
    state: &AppState,
    ws: Uuid,
    kind: Kind,
    direction: Direction,
    price: Option<f64>,
) -> Result<PartView, ApiError> {
    let limits = budget::limits(&state.pool, &state.config, ws, kind, direction).await?;
    let usage = budget::usage(&state.pool, ws, kind, direction).await?;
    let effective = limits.effective();
    Ok(PartView {
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

async fn view(state: &AppState, ws: Uuid) -> Result<BudgetView, ApiError> {
    let price = state
        .config
        .llm
        .as_ref()
        .and_then(|l| l.embedding_price_per_million_tokens);
    Ok(BudgetView {
        llm_configured: state.config.llm.is_some(),
        part: part(state, ws, Kind::Embedding, Direction::Input, price).await?,
    })
}

async fn answer_view(state: &AppState, ws: Uuid) -> Result<AnswerBudgetView, ApiError> {
    let llm = state.config.llm.as_ref();
    let kind = Kind::VisitorAnswers;
    // 単価は、ワークスペースの上書きが効いていれば上書きのもの（Q26）
    let (input_price, output_price) =
        crate::workspace_llm::chat_prices(&state.pool, &state.config, ws).await?;
    Ok(AnswerBudgetView {
        chat_configured: llm.is_some_and(|l| l.chat_model.is_some()),
        input: part(state, ws, kind, Direction::Input, input_price).await?,
        output: part(state, ws, kind, Direction::Output, output_price).await?,
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

pub async fn get_answer(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<AnswerBudgetView>, ApiError> {
    let ws = parse(&ws)?;
    holder(&state, &current, ws, Permission::ManageLlm).await?;
    Ok(Json(answer_view(&state, ws).await?))
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetBudget {
    /// 上限（トークン数）。null は上限なし（天井がそのまま効く）。
    tokens_per_month: Option<i64>,
    tokens_per_day: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetAnswerBudget {
    /// 入力・出力の上限。項目がなければ変えない。
    #[serde(default)]
    input: Option<SetBudget>,
    #[serde(default)]
    output: Option<SetBudget>,
}

/// 上限を確かめる（負でない、天井の範囲）。天井は使う瞬間にも照合する。
async fn check_budget(
    state: &AppState,
    ws: Uuid,
    kind: Kind,
    direction: Direction,
    req: SetBudget,
) -> Result<(), ApiError> {
    if [req.tokens_per_month, req.tokens_per_day]
        .iter()
        .any(|v| v.is_some_and(|t| t < 0))
    {
        return Err(ApiError::bad_request(
            "`tokensPerMonth` and `tokensPerDay` must not be negative.",
        ));
    }
    let ceiling = budget::ceiling(&state.pool, &state.config, ws, kind, direction).await?;
    if req.tokens_per_month.is_some_and(|t| t > ceiling.per_month)
        || req.tokens_per_day.is_some_and(|t| t > ceiling.per_day)
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "budget_exceeds_ceiling",
            "The budget exceeds the limit set by the server administrator.",
        ));
    }
    Ok(())
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
    check_budget(&state, ws, Kind::Embedding, Direction::Input, req).await?;
    budget::set_limits(
        &state.pool,
        ws,
        Kind::Embedding,
        Direction::Input,
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

pub async fn set_answer(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    JsonBody(req): JsonBody<SetAnswerBudget>,
) -> Result<Json<AnswerBudgetView>, ApiError> {
    let ws = parse(&ws)?;
    holder(&state, &current, ws, Permission::ManageLlm).await?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    let parts = [
        (Direction::Input, req.input),
        (Direction::Output, req.output),
    ];
    // 両方を確かめてから変える（片方だけが変わらないように）
    for (direction, set) in parts {
        if let Some(set) = set {
            check_budget(&state, ws, Kind::VisitorAnswers, direction, set).await?;
        }
    }
    let mut changed = false;
    for (direction, set) in parts {
        if let Some(set) = set {
            budget::set_limits(
                &state.pool,
                ws,
                Kind::VisitorAnswers,
                direction,
                set.tokens_per_month,
                set.tokens_per_day,
                current.user_id,
            )
            .await?;
            changed = true;
        }
    }
    if changed {
        audit::record(
            &state.pool,
            Some(ws),
            Some(current.user_id),
            action::BUDGET_CHANGED,
            &[ws],
        )
        .await?;
    }
    Ok(Json(answer_view(&state, ws).await?))
}
