//! 質問応答（回答API。09-qa-chatbot.md、10-llm-and-budget.md「予算」、13-security.md「チャットボットへの悪用」、M4）。
//!
//! 配信APIの `answer`（まとめて返す）と `answerStream`（判定先行のストリーミング）から呼ぶ。流れ:
//! 1. 質問文の長さを確かめる（[`MAX_QUESTION_CHARS`] を超えたら拒否。INV-QA-12）。
//! 2. 質問文をマスクする（外部に送る前。埋め込みにも LLM にも、マスクした文だけを送る。INV-QA-15、Q169）。
//! 3. 根拠の断片を選ぶ（共通の層を通り、質問のたびに今の状態と照合する。`search::query::fragments`）。
//!    断片がなければ、LLMを呼ばずに「情報なし」。読み取りのトランザクションは、LLMを呼ぶ前に閉じる。
//! 4. 訪問者向けの回答の予算から、入力の見積もり（1文字＝1トークン）と出力の上限（[`MAX_OUTPUT_TOKENS`]）を確保する。
//!    足りなければLLMを呼ばずに予算切れ（INV-QA-05）。呼んだ後、使用量（なければ見積もり）で精算する。
//! 5. LLMの回答を `check` で検査しながら返す（宣言の確認・長さの打ち切り・引用の照合。INV-QA-01, 04, 13）。
//!
//! 質問文と回答の本文はログに出さない（原則7）。ログに残すのは、規則違反の種類だけ。

pub mod check;
pub mod format;
mod prompt;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc;
use uuid::Uuid;

use self::check::{Checker, Step, Verdict};
use crate::audit;
use crate::budget::{self, Direction, Kind, Reservation, Reserved};
use crate::content::freshness;
use crate::http::AppState;
use crate::llm::{Llm, LlmError};
use crate::schema::manifest::Manifest;
use crate::search::query::{self as search, Fragment};
use crate::workspace_db::{self, WorkspaceDb};

/// 質問文の長さの上限（文字。Unicode のコードポイント）。
pub const MAX_QUESTION_CHARS: usize = 500;
/// LLMに求める回答の長さの上限（トークン）。予算にも、この分を確保する。
pub const MAX_OUTPUT_TOKENS: u32 = 1024;
/// LLMに渡す断片の数。
const FRAGMENTS: usize = 6;
/// ベクトル検索の候補の、コサイン類似度の足切り（ゆるい足切り。最終の判断はLLM）。
const MIN_SIMILARITY: f64 = 0.2;
/// 「情報なし」の文言がロケールにも英語にもないときの文。
const NO_INFORMATION_FALLBACK: &str = "No relevant information was found.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Answered,
    NoInformation,
    /// 本文の途中で規則違反（宣言していない番号の引用）を検知した（ストリーミングだけ。配信した後なので、最終判定で示す）。
    ViolationDetected,
}

impl Status {
    /// GraphQL の列挙の値。
    pub fn name(self) -> &'static str {
        match self {
            Self::Answered => "ANSWERED",
            Self::NoInformation => "NO_INFORMATION",
            Self::ViolationDetected => "VIOLATION_DETECTED",
        }
    }
}

/// 回答の出典（記事の該当セクション）。
#[derive(Debug, Clone)]
pub struct Source {
    pub entry_id: Uuid,
    pub locale: String,
    pub url: Option<String>,
    pub anchor: Option<String>,
    pub breadcrumb: Vec<String>,
    pub version_id: Uuid,
    pub last_reviewed_at: Option<String>,
    pub review_expired: bool,
}

#[derive(Debug, Clone)]
pub struct Answer {
    pub status: Status,
    /// 回答の本文（Markdown）。「情報なし」ならワークスペースの定型文。
    pub text: String,
    pub truncated: bool,
    pub sources: Vec<Source>,
}

/// ストリーミングのイベント。まとめて返すときは `Final` だけを使う。
#[derive(Debug, Clone)]
pub enum Event {
    Verdict {
        answerable: bool,
        sources: Vec<Source>,
    },
    Delta(String),
    Final(Answer),
}

/// 回答できなかった理由（GraphQL のエラーの `extensions.code`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    QuestionEmpty,
    QuestionTooLong,
    /// ロケールが、ワークスペースのテンプレートのロケールでない。
    LocaleNotSupported,
    /// 回答の生成の設定（`[llm] chat_model`）がない。
    Unavailable,
    BudgetExhausted,
    /// LLMの呼び出しに失敗した（タイムアウト・429・5xx・途中で切れた応答など）。リトライはしない。
    LlmUnavailable,
    Internal,
}

impl Refusal {
    pub fn code(self) -> &'static str {
        match self {
            Self::QuestionEmpty => "QUESTION_EMPTY",
            Self::QuestionTooLong => "QUESTION_TOO_LONG",
            Self::LocaleNotSupported => "LOCALE_NOT_SUPPORTED",
            Self::Unavailable => "ANSWER_UNAVAILABLE",
            Self::BudgetExhausted => "ANSWER_BUDGET_EXHAUSTED",
            Self::LlmUnavailable => "LLM_UNAVAILABLE",
            Self::Internal => "INTERNAL_ERROR",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::QuestionEmpty => "The question is empty.",
            Self::QuestionTooLong => "The question is too long.",
            Self::LocaleNotSupported => "The locale is not one of this workspace's locales.",
            Self::Unavailable => "Answers are not configured on this server.",
            Self::BudgetExhausted => "The answer budget of this workspace is used up.",
            Self::LlmUnavailable => "The language model could not be reached. Try again later.",
            Self::Internal => "internal error",
        }
    }
}

/// 回答の依頼。
pub struct Request {
    pub workspace_id: Uuid,
    pub db: WorkspaceDb,
    pub manifest: Arc<Manifest>,
    pub question: String,
    pub locale: String,
    /// ストリーミングか（まとめて返すときは、本文の断片を送らない）。
    pub streaming: bool,
}

/// 回答を始める。イベントを順に受け取る（最後は `Final` か、エラー）。受け取る側を落とすと、LLMの応答を読むのをやめる。
pub fn start(app: AppState, req: Request) -> mpsc::Receiver<Result<Event, Refusal>> {
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(async move {
        if let Err(refusal) = run(&app, &req, &tx).await {
            let _ = tx.send(Err(refusal)).await;
        }
    });
    rx
}

/// 根拠の断片と、出典に要る記事の情報。
struct Grounds {
    fragments: Vec<Fragment>,
    /// 記事ごとの（公開URL、公開中の版、最終確認日、確認期限切れか）。
    entries: HashMap<Uuid, (Option<String>, Uuid, Option<String>, bool)>,
    no_information: String,
}

impl Grounds {
    fn source(&self, n: usize) -> Option<Source> {
        let f = self.fragments.get(n.checked_sub(1)?)?;
        let (url, version_id, last, expired) = self.entries.get(&f.entry_id)?.clone();
        Some(Source {
            entry_id: f.entry_id,
            locale: f.locale.clone(),
            url,
            anchor: f.anchor.clone(),
            breadcrumb: f.breadcrumb.clone(),
            version_id,
            last_reviewed_at: last,
            review_expired: expired,
        })
    }

    fn sources(&self, numbers: &[usize]) -> Vec<Source> {
        numbers.iter().filter_map(|&n| self.source(n)).collect()
    }

    fn no_information(&self) -> Answer {
        Answer {
            status: Status::NoInformation,
            text: self.no_information.clone(),
            truncated: false,
            sources: Vec::new(),
        }
    }
}

fn internal(e: anyhow::Error) -> Refusal {
    tracing::error!("answer failed: {e:#}");
    Refusal::Internal
}

type Out = mpsc::Sender<Result<Event, Refusal>>;

async fn run(app: &AppState, req: &Request, out: &Out) -> Result<(), Refusal> {
    let question = req.question.trim();
    if question.is_empty() {
        return Err(Refusal::QuestionEmpty);
    }
    if question.chars().count() > MAX_QUESTION_CHARS {
        return Err(Refusal::QuestionTooLong);
    }
    // ロケールはプロンプトにも入るので、テンプレートのロケールだけを受け付ける（リクエストの値をそのまま入れない）
    let Some(locale) = req.manifest.locales.iter().find(|l| **l == req.locale) else {
        return Err(Refusal::LocaleNotSupported);
    };
    let (Some(llm_config), Some(egress)) = (app.config.llm.as_ref(), app.egress.as_ref()) else {
        return Err(Refusal::Unavailable);
    };
    let Some(model) = llm_config.chat_model.clone() else {
        return Err(Refusal::Unavailable);
    };
    // 外部（埋め込み・LLM）に送る前にマスクする
    let masked = crate::mask::mask(question);
    let grounds = grounds(app, req, &masked).await.map_err(internal)?;
    if grounds.fragments.is_empty() {
        // 根拠がなければ、LLMを呼ばない（予算も使わない）
        let _ = out
            .send(Ok(Event::Verdict {
                answerable: false,
                sources: Vec::new(),
            }))
            .await;
        let _ = out.send(Ok(Event::Final(grounds.no_information()))).await;
        return Ok(());
    }
    let fragments: Vec<prompt::PromptFragment> = grounds
        .fragments
        .iter()
        .map(|f| prompt::PromptFragment {
            text: fragment_text(f),
            review_expired: grounds.entries.get(&f.entry_id).is_some_and(|e| e.3),
            // 断片のロケールは、質問のロケールか既定のロケール（検索で絞ったもの）。印にはテンプレートの値を使う
            other_locale: (f.locale != *locale)
                .then(|| req.manifest.default_locale.clone())
                .filter(|d| *d == f.locale),
        })
        .collect();
    let messages = prompt::messages(locale, &masked, &fragments);
    let input_estimate = prompt::estimate_tokens(&messages);

    let pool = &app.pool;
    let ws = req.workspace_id;
    let (input, output) = reserve(app, ws, input_estimate).await?;
    let llm = Llm::new(egress.clone(), llm_config.current());
    let mut stream = match llm.chat_stream(&model, &messages, MAX_OUTPUT_TOKENS).await {
        Ok(s) => s,
        Err(e) => {
            let charged = if budget::not_charged(e) {
                0
            } else {
                input_estimate
            };
            settle(pool, &input, charged, &output, 0).await;
            log_llm_error(e);
            return Err(Refusal::LlmUnavailable);
        }
    };

    let mut checker = Checker::new(grounds.fragments.len());
    let mut received_chars: i64 = 0;
    let mut refused = false;
    let mut failure = None;
    // LLMの応答を終わりまで読んだか（途中でやめたら、相手は出力を作り続けたかもしれない）
    let mut completed = false;
    'read: loop {
        let text = match stream.next_text().await {
            Ok(Some(t)) => t,
            Ok(None) => {
                completed = true;
                break;
            }
            Err(e) => {
                failure = Some(e);
                break;
            }
        };
        received_chars += i64::try_from(text.chars().count()).unwrap_or(i64::MAX);
        for step in checker.push(&text) {
            if !send_step(&grounds, req.streaming, out, step, &mut refused).await {
                // 受け取る側がいない（訪問者が離れた）。読むのをやめる
                break 'read;
            }
        }
        if refused || checker.truncated() {
            break;
        }
    }
    let (input_used, output_used) = match stream.usage() {
        Some(u) => (
            i64::try_from(u.prompt_tokens).unwrap_or(i64::MAX),
            i64::try_from(u.completion_tokens).unwrap_or(i64::MAX),
        ),
        // 使用量がわからなければ、多めに数える（Q138）。途中でやめたときは、出力の上限まで使ったものとする
        None if completed => (input_estimate, received_chars),
        None => (
            input_estimate,
            i64::from(MAX_OUTPUT_TOKENS).max(received_chars),
        ),
    };
    settle(pool, &input, input_used, &output, output_used).await;
    if let Some(e) = failure {
        log_llm_error(e);
        return Err(Refusal::LlmUnavailable);
    }

    let (early, verdict) = checker.finish();
    if let Some(step) = early {
        send_step(&grounds, req.streaming, out, step, &mut refused).await;
    }
    let answer = match verdict {
        Verdict::NoInformation => grounds.no_information(),
        Verdict::Answered {
            text,
            cited,
            violation,
        } => {
            if violation {
                tracing::warn!(workspace = %ws, kind = "undeclared_citation", "answer broke the citation rule");
            }
            Answer {
                // まとめて返すときは、取り除いた後の本文だけを返すので違反は外に出ていない
                status: if violation && req.streaming {
                    Status::ViolationDetected
                } else {
                    Status::Answered
                },
                text,
                truncated: checker.truncated(),
                sources: grounds.sources(&cited),
            }
        }
    };
    let _ = out.send(Ok(Event::Final(answer))).await;
    Ok(())
}

/// 検査の段階をイベントにして送る。受け取る側がいなければ false。
async fn send_step(
    grounds: &Grounds,
    streaming: bool,
    out: &Out,
    step: Step,
    refused: &mut bool,
) -> bool {
    let event = match step {
        Step::Answerable(sources) => Event::Verdict {
            answerable: true,
            sources: grounds.sources(&sources),
        },
        Step::Refused => {
            *refused = true;
            Event::Verdict {
                answerable: false,
                sources: Vec::new(),
            }
        }
        Step::Delta(text) if streaming => Event::Delta(text),
        Step::Delta(_) | Step::Full => return true,
    };
    out.send(Ok(event)).await.is_ok()
}

fn log_llm_error(e: LlmError) {
    tracing::warn!(code = e.code(), status = ?e.status(), "answer LLM call failed");
}

/// プロンプトに入れる断片の文字列（パンくずと本文）。
fn fragment_text(f: &Fragment) -> String {
    if f.breadcrumb.is_empty() {
        f.content.clone()
    } else {
        format!("{}: {}", f.breadcrumb.join(" > "), f.content)
    }
}

/// 根拠の断片と出典の情報を、読み取りのトランザクションで集める（LLMを呼ぶ前に閉じる）。
async fn grounds(app: &AppState, req: &Request, masked: &str) -> Result<Grounds> {
    let mut tx = workspace_db::begin_owned(&app.pool, &req.db).await?;
    let services = search::Services {
        pool: &app.pool,
        config: &app.config,
        egress: app.egress.as_ref(),
        cache: &app.search_cache,
    };
    let fragments = search::fragments(
        &services,
        &mut tx,
        req.workspace_id,
        &req.manifest,
        masked,
        &req.locale,
        FRAGMENTS,
        MIN_SIMILARITY,
    )
    .await?;
    let mut entries = HashMap::new();
    for f in &fragments {
        if entries.contains_key(&f.entry_id) {
            continue;
        }
        let row: Option<(Option<String>, Uuid, String)> = sqlx::query_as(
            "SELECT url, published_version_id, content_type FROM entries
             WHERE id = $1 AND deleted_at IS NULL AND published_version_id IS NOT NULL",
        )
        .bind(f.entry_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((url, version_id, content_type)) = row else {
            continue;
        };
        let days = req
            .manifest
            .content_type(&content_type)
            .and_then(|ct| ct.review.as_ref())
            .map(|r| r.interval_days);
        let fresh = freshness::get(&mut tx, days, f.entry_id).await?;
        let (last, expired) =
            fresh.map_or((None, false), |f| (f.last_verified_at, f.review_expired));
        entries.insert(
            f.entry_id,
            (url.filter(|u| !u.is_empty()), version_id, last, expired),
        );
    }
    // 出典の情報が取れなかった断片（同じトランザクションの中なので、ふつうはない）は使わない
    let fragments: Vec<Fragment> = fragments
        .into_iter()
        .filter(|f| entries.contains_key(&f.entry_id))
        .collect();
    let no_information: Option<String> = sqlx::query_scalar(
        "SELECT no_information FROM answer_phrases WHERE locale = $1 OR locale = 'en'
         ORDER BY (locale = $1) DESC LIMIT 1",
    )
    .bind(&req.locale)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Grounds {
        fragments,
        entries,
        no_information: no_information.unwrap_or_else(|| NO_INFORMATION_FALLBACK.to_owned()),
    })
}

/// 入力の見積もりと出力の上限を確保する。どちらかが足りなければ、もう片方を戻して予算切れ。
async fn reserve(
    app: &AppState,
    ws: Uuid,
    input_estimate: i64,
) -> Result<(Reservation, Reservation), Refusal> {
    let pool = &app.pool;
    let config = &app.config;
    let kind = Kind::VisitorAnswers;
    let exhausted = |first: bool| async move {
        if first {
            audit::record(
                pool,
                Some(ws),
                None,
                audit::action::ANSWER_BUDGET_EXHAUSTED,
                &[ws],
            )
            .await
            .map_err(internal)?;
        }
        Err::<(Reservation, Reservation), Refusal>(Refusal::BudgetExhausted)
    };
    let input = match budget::reserve(pool, config, ws, kind, Direction::Input, input_estimate)
        .await
        .map_err(internal)?
    {
        Reserved::Ok(r) => r,
        Reserved::Exhausted { first } => return exhausted(first).await,
    };
    let output_max = i64::from(MAX_OUTPUT_TOKENS);
    match budget::reserve(pool, config, ws, kind, Direction::Output, output_max)
        .await
        .map_err(internal)?
    {
        Reserved::Ok(output) => Ok((input, output)),
        Reserved::Exhausted { first } => {
            budget::settle(pool, &input, 0).await.map_err(internal)?;
            exhausted(first).await
        }
    }
}

/// 精算する（失敗はログに残すだけ。精算されなかった確保は使ったものとして残り、上限を超える側には倒れない）。
async fn settle(
    pool: &sqlx::PgPool,
    input: &Reservation,
    input_used: i64,
    output: &Reservation,
    output_used: i64,
) {
    for (r, used) in [(input, input_used), (output, output_used)] {
        if let Err(e) = budget::settle(pool, r, used).await {
            tracing::error!("answer budget settlement failed: {e:#}");
        }
    }
}
