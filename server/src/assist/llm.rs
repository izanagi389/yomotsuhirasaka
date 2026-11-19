//! 支援の LLM の呼び出し（「編集者向けの支援」の予算の範囲で。10-llm-and-budget.md「予算」、Q70, Q138）。
//!
//! - 接続先はワークスペースの上書きか、インスタンスの設定（Q26）。チャットのモデルがなければ呼ばない。
//! - 呼ぶ前に、入力の見積もり（1文字＝1トークン）と出力の上限を確保する。足りなければ呼ばず、尽きた期間を返す
//!   （期間ごとに1回、ワークスペースの管理者に知らせる）。呼んだあと、使用量（なければ文字数）で精算する。
//! - 画像を読む呼び出し（代替テキストの案。2026-11-17）は、画像を読むモデル（[`call_vision`]）で行う。モデルがなければ呼ばない。
//!   画像の入力は、1枚あたり [`IMAGE_INPUT_TOKENS`] トークンとして見積もって確保する（精算は相手の使用量で）。
//! - 温度は [`TEMPERATURE`]。
//! - 失敗の中身（相手の応答や送った文）はログに書かない。

use anyhow::Result;
use uuid::Uuid;

use crate::audit;
use crate::budget::{self, Direction, Kind, Period, Reserved};
use crate::http::AppState;
use crate::llm::{ChatMessage, Llm};

/// 支援の呼び出しの温度。判定と抜き出しが仕事なので、揺れないように 0 にする（同じ入力には同じ答え。言語の揺れも減る）。
pub const TEMPERATURE: f32 = 0.0;

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

/// 画像1枚の入力の見積もり（トークン数）。主な提供会社の、大きな画像1枚の上限ほどの値にする。
pub const IMAGE_INPUT_TOKENS: i64 = 1600;

pub async fn call(
    state: &AppState,
    ws: Uuid,
    messages: &[ChatMessage],
    max_tokens: u32,
    max_chars: usize,
) -> Result<Called> {
    call_inner(state, ws, messages, &[], max_tokens, max_chars).await
}

/// 画像（`data:` の URL）を付けて、画像を読むモデルで呼ぶ。
pub async fn call_vision(
    state: &AppState,
    ws: Uuid,
    messages: &[ChatMessage],
    images: &[String],
    max_tokens: u32,
    max_chars: usize,
) -> Result<Called> {
    call_inner(state, ws, messages, images, max_tokens, max_chars).await
}

/// 画像を読むモデルが使えるか（ワークスペースの上書きか、インスタンスの設定にあるか）。
pub async fn vision_available(state: &AppState, ws: Uuid) -> Result<bool> {
    Ok(
        crate::workspace_llm::effective(&state.pool, &state.keys, &state.config, ws)
            .await?
            .is_some_and(|(c, _)| c.vision_model.is_some()),
    )
}

async fn call_inner(
    state: &AppState,
    ws: Uuid,
    messages: &[ChatMessage],
    images: &[String],
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
    let model = if images.is_empty() {
        llm_config.chat_model.clone()
    } else {
        llm_config.vision_model.clone()
    };
    let Some(model) = model else {
        return Ok(Called::Failed);
    };
    let image_tokens =
        IMAGE_INPUT_TOKENS.saturating_mul(i64::try_from(images.len()).unwrap_or(i64::MAX));
    let input_estimate =
        crate::answer::prompt::estimate_tokens(messages).saturating_add(image_tokens);
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
        let mut stream = llm
            .chat_stream_with_images(&model, messages, images, max_tokens, Some(TEMPERATURE))
            .await?;
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
