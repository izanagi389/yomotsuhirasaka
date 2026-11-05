//! 管理API: 回答のテスト（09-qa-chatbot.md「根拠と判定」「語り口」、M4）。
//!
//! | メソッド | パス | 内容 | 誰が |
//! |---|---|---|---|
//! | POST | /api/admin/workspaces/{ws}/answer-test | テスト用の質問に答え、検索された断片・点数・判定の宣言・LLMの出力・最終の結果を返す | 重い権限 manageLlm を持つ人・サーバー管理者 |
//!
//! - 本文は `{question, locale, tone?}`。`tone`（丁寧さ・長さ・自由記述）を渡すと、保存する前の語り口で試せる。
//! - 拒否する語・マスク・「訪問者向けの回答」の予算・回答の検査（引用・URL・長さ）は、本番と同じに効く。
//!   回答キャッシュは読みも書きもせず、質問ログには質問文も件数も残さない。訪問者の回数制限は使わず、人ごとに1分
//!   [`PER_MINUTE`] 回まで。監査ログには残さない。
//! - 回答できなかった理由（予算切れ・拒否する語など）は、200 の `error.code` で返す（配信APIと同じコード）。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AppState;
use super::admin_entries::{db, parse, readable};
use super::admin_publishing::holder;
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::access::Permission;
use crate::answer::settings::{Length, MAX_FREE_PROMPT_CHARS, Politeness, Tone};
use crate::answer::{self, Event, Status};
use crate::delivery::limiter::Subject;

/// 人ごとの1分あたりの回数。
pub const PER_MINUTE: u32 = 10;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FreePrompt {
    enabled: bool,
    text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToneInput {
    politeness: Politeness,
    length: Length,
    free_prompt: FreePrompt,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TestRequest {
    question: String,
    locale: String,
    #[serde(default)]
    tone: Option<ToneInput>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FragmentView {
    n: usize,
    entry_id: Uuid,
    locale: String,
    anchor: Option<String>,
    breadcrumb: Vec<String>,
    excerpt: String,
    score: f64,
    vector_similarity: Option<f64>,
    keyword_score: Option<f64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclarationView {
    answerable: bool,
    sources: Vec<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceView {
    citation: usize,
    entry_id: Uuid,
    locale: String,
    url: Option<String>,
    anchor: Option<String>,
    breadcrumb: Vec<String>,
    version_id: Uuid,
    review_expired: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultView {
    /// `ANSWERED`・`NO_INFORMATION`・`VIOLATION_DETECTED`（配信APIと同じ）。
    status: &'static str,
    text: String,
    truncated: bool,
    sources: Vec<SourceView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorView {
    code: &'static str,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TestView {
    /// マスクした後の質問文（断る前に止まったときも、マスクの結果は見せる）。
    masked_question: String,
    fragments: Vec<FragmentView>,
    declaration: Option<DeclarationView>,
    raw_output: Option<String>,
    result: Option<ResultView>,
    error: Option<ErrorView>,
}

fn tone_of(input: ToneInput) -> Result<Tone, ApiError> {
    let text = input.free_prompt.text;
    if text.chars().count() > MAX_FREE_PROMPT_CHARS {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_answer_settings",
            format!(
                "The answer settings are not valid: freePrompt.text: at most {MAX_FREE_PROMPT_CHARS} characters"
            ),
        ));
    }
    if input.free_prompt.enabled && text.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_answer_settings",
            "The answer settings are not valid: freePrompt.text: must not be empty when enabled",
        ));
    }
    Ok(Tone {
        politeness: input.politeness,
        length: input.length,
        free_prompt: input.free_prompt.enabled.then_some(text),
    })
}

fn status_name(s: Status) -> &'static str {
    match s {
        Status::Answered => "ANSWERED",
        Status::NoInformation => "NO_INFORMATION",
        Status::ViolationDetected => "VIOLATION_DETECTED",
    }
}

pub async fn post(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    JsonBody(req): JsonBody<TestRequest>,
) -> Result<Json<TestView>, ApiError> {
    let ws = parse(&ws)?;
    holder(&state, &current, ws, Permission::ManageLlm).await?;
    let tone = req.tone.map(tone_of).transpose()?;
    if let Err(retry) = state.delivery_limiter.check(
        Subject::AnswerTest {
            workspace: ws,
            user: current.user_id,
        },
        PER_MINUTE,
    ) {
        return Err(ApiError::too_many(
            "answer_test_rate_limited",
            "Too many answer tests. Try again later.",
            retry,
        ));
    }
    let applied = readable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut out = TestView {
        masked_question: crate::mask::mask(req.question.trim()),
        ..TestView::default()
    };
    let mut rx = answer::start(
        state.clone(),
        answer::Request {
            workspace_id: ws,
            db,
            manifest: std::sync::Arc::new(applied.manifest),
            question: req.question,
            locale: req.locale,
            streaming: false,
            test: Some(answer::Test { tone }),
        },
    );
    while let Some(item) = rx.recv().await {
        match item {
            Ok(Event::Trace(t)) => {
                out.masked_question = t.masked_question;
                out.fragments = t
                    .fragments
                    .into_iter()
                    .map(|f| FragmentView {
                        n: f.n,
                        entry_id: f.entry_id,
                        locale: f.locale,
                        anchor: f.anchor,
                        breadcrumb: f.breadcrumb,
                        excerpt: f.excerpt,
                        score: f.score,
                        vector_similarity: f.vector_similarity,
                        keyword_score: f.keyword_score,
                    })
                    .collect();
                out.declaration = t.declaration.map(|sources| DeclarationView {
                    answerable: !sources.is_empty(),
                    sources,
                });
                out.raw_output = Some(t.raw_output);
            }
            Ok(Event::Final(a)) => {
                out.result = Some(ResultView {
                    status: status_name(a.status),
                    text: a.text,
                    truncated: a.truncated,
                    sources: a
                        .sources
                        .into_iter()
                        .map(|s| SourceView {
                            citation: s.citation,
                            entry_id: s.entry_id,
                            locale: s.locale,
                            url: s.url,
                            anchor: s.anchor,
                            breadcrumb: s.breadcrumb,
                            version_id: s.version_id,
                            review_expired: s.review_expired,
                        })
                        .collect(),
                });
            }
            Ok(Event::Verdict { .. } | Event::Delta(_)) => {}
            Err(r) => out.error = Some(ErrorView { code: r.code() }),
        }
    }
    Ok(Json(out))
}
