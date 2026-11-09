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

pub mod cache;
pub mod check;
pub mod format;
pub mod log;
pub(crate) mod prompt;
pub mod refuse;
pub mod settings;
pub mod urls;

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
pub(crate) const NO_INFORMATION_FALLBACK: &str = "No relevant information was found.";

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
    /// 本文の引用記号 `[n]` の番号（プロンプトの断片の番号）。本文と出典を結び付ける。
    pub citation: usize,
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
    /// 回答キャッシュから返したか。
    pub cached: bool,
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
    /// テストのときだけ、最終判定の前に1回（検索された断片・判定の宣言・LLMの出力）。配信APIには来ない。
    Trace(Box<Trace>),
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
    /// ワークスペースが答えないと決めた質問（拒否する語・組み込みの種類。INV-QA-16）。
    QuestionRefused,
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
            Self::QuestionRefused => "QUESTION_REFUSED",
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
            Self::QuestionRefused => "This workspace does not answer this question.",
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
    /// 管理画面のテスト画面からの依頼か（配信APIからは None）。
    pub test: Option<Test>,
}

/// 管理画面の回答のテスト画面（09-qa-chatbot.md「根拠と判定」「語り口」）。拒否する語・マスク・予算・回答の検査は本番と同じに
/// 効かせ、回答キャッシュは読みも書きもせず、質問ログには質問文も件数も残さない。
#[derive(Debug, Clone, Default)]
pub struct Test {
    /// 保存する前の語り口で試す（None なら保存済みの語り口）。
    pub tone: Option<settings::Tone>,
}

/// テストで見せる、検索された断片（プロンプトの番号の順）。
#[derive(Debug, Clone, PartialEq)]
pub struct TraceFragment {
    /// プロンプトの番号（本文の `[n]` の n）。
    pub n: usize,
    pub entry_id: Uuid,
    pub locale: String,
    pub anchor: Option<String>,
    pub breadcrumb: Vec<String>,
    /// 本文の抜粋（[`TRACE_EXCERPT_CHARS`] 文字まで）。
    pub excerpt: String,
    pub score: f64,
    pub vector_similarity: Option<f64>,
    pub keyword_score: Option<f64>,
}

/// テストで見せるもの（最終判定の前に1回送る）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Trace {
    /// マスクした後の質問文（LLMと埋め込みに送ったもの）。
    pub masked_question: String,
    pub fragments: Vec<TraceFragment>,
    /// LLMの判定の宣言（答えられるなら使う番号、答えられないなら空。宣言を読めなければ None）。
    pub declaration: Option<Vec<usize>>,
    /// LLMの出力そのもの（検査の前。[`TRACE_OUTPUT_CHARS`] 文字まで）。
    pub raw_output: String,
}

/// テストで見せる抜粋と、LLMの出力の文字数の上限。
pub const TRACE_EXCERPT_CHARS: usize = 300;
pub const TRACE_OUTPUT_CHARS: usize = 8000;

/// 回答を始める。イベントを順に受け取る（最後は `Final` か、エラー）。受け取る側を落とすと、LLMの応答を読むのをやめる。
pub fn start(app: AppState, req: Request) -> mpsc::Receiver<Result<Event, Refusal>> {
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(async move {
        if let Err(refusal) = run(&app, &req, &tx).await {
            // 断った依頼は、日ごとの件数だけを残す（質問文は残さない）
            let outcome = if refusal == Refusal::QuestionRefused {
                log::Outcome::Refused
            } else {
                log::Outcome::Error
            };
            note(&app, &req, outcome, None).await;
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
            citation: n,
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

    /// LLMに渡した断片の記事（キャッシュの照合に使う）。
    fn cache_grounds(&self) -> Vec<cache::Ground> {
        self.fragments
            .iter()
            .filter_map(|f| {
                let (_, version_id, _, _) = self.entries.get(&f.entry_id)?;
                Some(cache::Ground {
                    entry_id: f.entry_id,
                    version_id: *version_id,
                    locale: f.locale.clone(),
                })
            })
            .collect()
    }

    fn no_information(&self) -> Answer {
        Answer {
            status: Status::NoInformation,
            text: self.no_information.clone(),
            truncated: false,
            sources: Vec::new(),
            cached: false,
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
    // 答えない質問（拒否する語・組み込みの種類）。検索・埋め込み・LLM・回答キャッシュのどれよりも前に照合する
    if load_refusal(app, req)
        .await
        .map_err(internal)?
        .refuses(question)
    {
        return Err(Refusal::QuestionRefused);
    }
    let Some(egress) = app.egress.as_ref() else {
        return Err(Refusal::Unavailable);
    };
    // ワークスペースの上書き（許されていて、許可の一覧にある接続先なら）か、インスタンスの設定（Q26）
    let Some((llm_config, endpoint)) =
        crate::workspace_llm::effective(&app.pool, &app.keys, &app.config, req.workspace_id)
            .await
            .map_err(internal)?
    else {
        return Err(Refusal::Unavailable);
    };
    let Some(model) = llm_config.chat_model.clone() else {
        return Err(Refusal::Unavailable);
    };
    // 外部（埋め込み・LLM）に送る前にマスクする
    let masked = crate::mask::mask(question);
    // 回答キャッシュ（出典を今の状態と照合してから返す。予算・埋め込み・LLMは使わない）
    // 語り口（回答キャッシュのキーとプロンプトに使う）
    let tone = match req.test.as_ref().and_then(|t| t.tone.clone()) {
        Some(t) => t,
        None => load_tone(app, req).await.map_err(internal)?,
    };
    // 回答キャッシュの設定には、接続先の名前とモデルを入れる（上書きを変えたら、前のキャッシュに当たらない）
    let cache_key = cache_key(
        app,
        req,
        locale,
        &format!("{endpoint}\u{0}{model}"),
        &tone,
        &masked,
    )
    .map_err(internal)?;
    // テストのときは、回答キャッシュを読まない（毎回 LLM を呼ぶ）
    let hit = if req.test.is_none() {
        cached(app, req, &cache_key).await.map_err(internal)?
    } else {
        None
    };
    if let Some(hit) = hit {
        let _ = out
            .send(Ok(Event::Verdict {
                answerable: true,
                sources: hit.sources.clone(),
            }))
            .await;
        if req.streaming {
            let _ = out.send(Ok(Event::Delta(hit.text.clone()))).await;
        }
        note(app, req, log::Outcome::Answered, None).await;
        let _ = out
            .send(Ok(Event::Final(Answer {
                status: Status::Answered,
                text: hit.text,
                truncated: hit.truncated,
                sources: hit.sources,
                cached: true,
            })))
            .await;
        return Ok(());
    }
    let grounds = grounds(app, req, &masked).await.map_err(internal)?;
    let mut trace = req.test.as_ref().map(|_| Trace {
        masked_question: masked.clone(),
        fragments: grounds
            .fragments
            .iter()
            .enumerate()
            .map(|(i, f)| TraceFragment {
                n: i + 1,
                entry_id: f.entry_id,
                locale: f.locale.clone(),
                anchor: f.anchor.clone(),
                breadcrumb: f.breadcrumb.clone(),
                excerpt: f.content.chars().take(TRACE_EXCERPT_CHARS).collect(),
                score: f.score,
                vector_similarity: f.vector_similarity,
                keyword_score: f.keyword_score,
            })
            .collect(),
        declaration: None,
        raw_output: String::new(),
    });
    if grounds.fragments.is_empty() {
        // 根拠がなければ、LLMを呼ばない（予算も使わない）
        let _ = out
            .send(Ok(Event::Verdict {
                answerable: false,
                sources: Vec::new(),
            }))
            .await;
        note(app, req, log::Outcome::NoInformation, Some(&masked)).await;
        if let Some(t) = trace {
            let _ = out.send(Ok(Event::Trace(Box::new(t)))).await;
        }
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
    let messages = prompt::messages(locale, &masked, &fragments, &tone);
    let input_estimate = prompt::estimate_tokens(&messages);

    let pool = &app.pool;
    let ws = req.workspace_id;
    let (input, output) = reserve(app, ws, input_estimate).await?;
    let llm = Llm::new(egress.clone(), llm_config.clone());
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

    // 断片ごとの、本文に残してよいURLの手がかり（LLMに渡した断片の文字列と、記事の公開URL）
    let url_grounds = grounds
        .fragments
        .iter()
        .zip(&fragments)
        .map(|(f, p)| {
            let public = grounds
                .entries
                .get(&f.entry_id)
                .and_then(|e| e.0.as_deref());
            urls::Ground::new(&p.text, public)
        })
        .collect();
    let mut checker = Checker::new(url_grounds);
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
        if let Some(t) = trace.as_mut() {
            let room = TRACE_OUTPUT_CHARS.saturating_sub(t.raw_output.chars().count());
            t.raw_output.extend(text.chars().take(room));
        }
        for step in checker.push(&text) {
            note_declaration(trace.as_mut(), &step);
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

    let (rest, verdict) = checker.finish();
    for step in rest {
        note_declaration(trace.as_mut(), &step);
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
            let answer = Answer {
                // まとめて返すときは、取り除いた後の本文だけを返すので違反は外に出ていない
                status: if violation && req.streaming {
                    Status::ViolationDetected
                } else {
                    Status::Answered
                },
                text,
                truncated: checker.truncated(),
                sources: grounds.sources(&cited),
                cached: false,
            };
            // 回答キャッシュに保存する。違反を検知したもの・終わりまで読まなかったもの（訪問者が離れたなど。長さの上限で
            // 打ち切ったものは除く）は保存しない。保存できなくても、回答は返す
            if req.test.is_none()
                && !violation
                && (completed || checker.truncated())
                && let Err(e) =
                    remember(app, req, &cache_key, &answer, &grounds.cache_grounds()).await
            {
                tracing::warn!(workspace = %ws, "answer cache store failed: {e:#}");
            }
            answer
        }
    };
    // 質問ログ（答えられなかった質問だけ文を残す）と日ごとの件数。最終判定を送る前に記録する
    if answer.status == Status::NoInformation {
        note(app, req, log::Outcome::NoInformation, Some(&masked)).await;
    } else {
        note(app, req, log::Outcome::Answered, None).await;
    }
    if let Some(t) = trace {
        let _ = out.send(Ok(Event::Trace(Box::new(t)))).await;
    }
    let _ = out.send(Ok(Event::Final(answer))).await;
    Ok(())
}

/// テストで見せる判定の宣言を覚える。
fn note_declaration(trace: Option<&mut Trace>, step: &Step) {
    if let Some(t) = trace {
        match step {
            Step::Answerable(sources) => t.declaration = Some(sources.clone()),
            Step::Refused => t.declaration = Some(Vec::new()),
            _ => {}
        }
    }
}

/// 回答の結果を質問ログに記録する。記録できなくても、回答は返す（ログには質問文を出さない）。
async fn note(app: &AppState, req: &Request, outcome: log::Outcome, masked: Option<&str>) {
    // テストのときは、質問文も件数も残さない
    if req.test.is_some() {
        return;
    }
    if let Err(e) = log::record(&app.pool, &req.db, outcome, &req.locale, masked).await {
        tracing::warn!(workspace = %req.workspace_id, "question log record failed: {e:#}");
    }
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

/// 回答キャッシュのキー（正規化してマスクした質問文と、ワークスペース・ロケール・設定から）。
fn cache_key(
    app: &AppState,
    req: &Request,
    locale: &str,
    model: &str,
    tone: &settings::Tone,
    masked: &str,
) -> Result<Vec<u8>> {
    let secret = app.keys.derive(cache::KEY_PURPOSE)?;
    Ok(cache::key(
        &secret,
        req.workspace_id,
        locale,
        &cache::settings(model, &req.manifest, &tone.fingerprint()),
        &cache::normalize(masked),
    ))
}

/// 答えない質問の決まりを読む（自分の短いトランザクションで）。
async fn load_refusal(app: &AppState, req: &Request) -> Result<refuse::Rules> {
    let mut tx = workspace_db::begin_owned(&app.pool, &req.db).await?;
    let rules = settings::refusal(&mut tx).await?;
    tx.commit().await?;
    Ok(rules)
}

/// 語り口を読む（自分の短いトランザクションで）。
async fn load_tone(app: &AppState, req: &Request) -> Result<settings::Tone> {
    let mut tx = workspace_db::begin_owned(&app.pool, &req.db).await?;
    let tone = settings::tone(&mut tx).await?;
    tx.commit().await?;
    Ok(tone)
}

/// 回答キャッシュを引く（自分の短いトランザクションで）。
async fn cached(app: &AppState, req: &Request, key: &[u8]) -> Result<Option<cache::Hit>> {
    let mut tx = workspace_db::begin_owned(&app.pool, &req.db).await?;
    let hit = cache::lookup(&mut tx, &req.manifest, key, &req.locale).await?;
    tx.commit().await?;
    Ok(hit)
}

/// 回答を回答キャッシュに保存する。
async fn remember(
    app: &AppState,
    req: &Request,
    key: &[u8],
    answer: &Answer,
    grounds: &[cache::Ground],
) -> Result<()> {
    let mut tx = workspace_db::begin_owned(&app.pool, &req.db).await?;
    cache::store(
        &mut tx,
        key,
        &answer.text,
        answer.truncated,
        &answer.sources,
        grounds,
    )
    .await?;
    tx.commit().await?;
    Ok(())
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
    let setup =
        crate::workspace_llm::embedding(&app.pool, &app.keys, &app.config, req.workspace_id)
            .await?;
    let tx = workspace_db::begin_owned(&app.pool, &req.db).await?;
    let services = search::Services {
        pool: &app.pool,
        config: &app.config,
        embedding: setup.as_ref().map(|s| &s.llm),
        egress: app.egress.as_ref(),
        cache: &app.search_cache,
    };
    let (mut tx, vector) =
        search::query_embedding(&services, &req.db, tx, req.workspace_id, masked).await?;
    let fragments = search::fragments(
        &services,
        &mut tx,
        &req.manifest,
        masked,
        &req.locale,
        FRAGMENTS,
        MIN_SIMILARITY,
        vector?,
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
        Reserved::Exhausted { first, .. } => return exhausted(first).await,
    };
    let output_max = i64::from(MAX_OUTPUT_TOKENS);
    match budget::reserve(pool, config, ws, kind, Direction::Output, output_max)
        .await
        .map_err(internal)?
    {
        Reserved::Ok(output) => Ok((input, output)),
        Reserved::Exhausted { first, .. } => {
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
