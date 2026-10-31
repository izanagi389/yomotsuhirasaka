//! 支援の LLM の呼び出し（「編集者向けの支援」の予算の範囲で。10-llm-and-budget.md「予算」、Q70, Q138）。
//!
//! - 接続先はワークスペースの上書きか、インスタンスの設定（Q26）。チャットのモデルがなければ呼ばない。
//! - 呼ぶ前に、入力の見積もり（1文字＝1トークン）と出力の上限を確保する。足りなければ呼ばず、尽きた期間を返す
//!   （期間ごとに1回、ワークスペースの管理者に知らせる）。呼んだあと、使用量（なければ文字数）で精算する。
//! - 失敗の中身（相手の応答や送った文）はログに書かない。

use anyhow::Result;
use uuid::Uuid;

use crate::audit;
use crate::budget::{self, Direction, Kind, Period, Reserved};
use crate::http::AppState;
use crate::llm::{ChatMessage, Llm};

/// 呼び出しの結果。
#[derive(Debug, PartialEq, Eq)]
pub enum Called {
    /// LLM の出力（`max_chars` 文字を超えたところで読むのをやめる）。CMS が検査してから使う。
    Reply(String),
    /// 予算が足りないので呼ばなかった。
    Exhausted(Period),
    /// 接続先がない、または呼んだが失敗した。
    Failed,
}

pub async fn call(
    state: &AppState,
    ws: Uuid,
    messages: &[ChatMessage],
    max_tokens: u32,
    max_chars: usize,
) -> Result<Called> {
    let pool = &state.pool;
    let config = &state.config;
    let (Some(egress), Some((llm_config, _))) = (
        state.egress.as_ref(),
        crate::workspace_llm::effective(pool, &state.keys, config, ws).await?,
    ) else {
        return Ok(Called::Failed);
    };
    let Some(model) = llm_config.chat_model.clone() else {
        return Ok(Called::Failed);
    };
    let input_estimate = crate::answer::prompt::estimate_tokens(messages);
    let exhausted = |first: bool, period: Period| async move {
        if first {
            audit::record(
                pool,
                Some(ws),
                None,
                audit::action::ASSIST_BUDGET_EXHAUSTED,
                &[ws],
            )
            .await?;
        }
        Ok(Called::Exhausted(period))
    };
    let kind = Kind::EditorAssist;
    let input =
        match budget::reserve(pool, config, ws, kind, Direction::Input, input_estimate).await? {
            Reserved::Ok(r) => r,
            Reserved::Exhausted { first, period } => return exhausted(first, period).await,
        };
    let output_max = i64::from(max_tokens);
    let output =
        match budget::reserve(pool, config, ws, kind, Direction::Output, output_max).await? {
            Reserved::Ok(r) => r,
            Reserved::Exhausted { first, period } => {
                budget::settle(pool, &input, 0).await?;
                return exhausted(first, period).await;
            }
        };
    let llm = Llm::new(egress.clone(), llm_config);
    let reply = async {
        let mut stream = llm.chat_stream(&model, messages, max_tokens).await?;
        let mut text = String::new();
        while let Some(t) = stream.next_text().await? {
            text.push_str(&t);
            if text.chars().count() > max_chars {
                break;
            }
        }
        Ok::<_, crate::llm::LlmError>((text, stream.usage()))
    }
    .await;
    match reply {
        Ok((text, usage)) => {
            let (input_used, output_used) = match usage {
                Some(u) => (
                    i64::try_from(u.prompt_tokens).unwrap_or(i64::MAX),
                    i64::try_from(u.completion_tokens).unwrap_or(i64::MAX),
                ),
                None => (
                    input_estimate,
                    i64::try_from(text.chars().count().max(1)).unwrap_or(i64::MAX),
                ),
            };
            budget::settle(pool, &input, input_used).await?;
            budget::settle(pool, &output, output_used).await?;
            Ok(Called::Reply(text))
        }
        Err(e) => {
            // 相手が処理を始める前の失敗だけ戻す（それ以外は確保した分を使ったものとして残す）
            if budget::not_charged(e) {
                budget::settle(pool, &input, 0).await?;
                budget::settle(pool, &output, 0).await?;
            }
            tracing::warn!(code = e.code(), "editor assist: LLM call failed");
            Ok(Called::Failed)
        }
    }
}
