//! 管理API: 編集者向けのAI支援（10-llm-and-budget.md「編集者向けのAI支援」、Q69, Q70、M4）。
//!
//! | メソッド | パス | 内容 | 誰が |
//! |---|---|---|---|
//! | GET | /api/admin/workspaces/{ws}/assist-settings | 支援を使うか（`enabled`） | 重い権限 manageLlm を持つ人・サーバー管理者 |
//! | PUT | /api/admin/workspaces/{ws}/assist-settings | 同じく、変える | 同上（再認証） |
//! | POST | /api/admin/workspaces/{ws}/entries/{id}/assist/conflicts | いまの下書きについて、矛盾の検知を始める（公開の確認画面） | 記事を編集できる人 |
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/assist/conflicts | いまの下書きについての結果 | 同上 |
//! | POST | /api/admin/workspaces/{ws}/entries/{id}/assist/stale | いまの下書きについて、見直しの手がかりを作る（ボタン） | 同上 |
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/assist/stale | その結果 | 同上 |
//! | POST | /api/admin/workspaces/{ws}/entries/{id}/assist/structure | いまの下書きについて、構成の確認を始める（ボタン。2026-11-17） | 同上（答えられなかった質問は、重い権限 viewQuestionLog と再認証がある人だけ） |
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/assist/structure | その結果と、始める前の見積もり | 同上 |
//! | POST | /api/admin/workspaces/{ws}/entries/{id}/assist/proofread | いまの下書きについて、校正を始める（ボタン。2026-11-17） | 記事を編集できる人 |
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/assist/proofread | その結果（いまの下書きで当てられるか）と、始める前の見積もり | 同上 |
//! | POST | /api/admin/workspaces/{ws}/question-log/candidates/{id}/outline | 候補から、書くべき記事の構成案を作る（ボタン） | 重い権限 viewQuestionLog を持つ人（再認証） |
//! | GET | /api/admin/workspaces/{ws}/question-log/candidates/{id}/outline | その結果 | 同上 |
//!
//! - 有効にすると、公開前の下書きが外部のLLMの提供会社に送られる（管理画面で注意を出す。Q177）。
//! - 変更は監査ログに残す（詳細は変えた後の値）。
//! - 矛盾の検知は非同期（`assist::conflicts`）。公開はこの結果を待たず、見もしない（INV-BUD-02）。
//! - 見直しの手がかりと構成案も非同期（`assist::tasks`）。構成案は質問文から作るので、質問ログの閲覧と同じく監査ログに残す。
//! - 構成の確認も非同期（`assist::tasks`・`assist::structure`）。答えられなかった質問を含める回は、同じく監査ログに残す。
//! - 校正も非同期（`assist::tasks`・`assist::proofread`）。
//!   どの支援も提案を返すだけで、下書きは変えない（INV-EDIT-02）。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Serialize;

use super::AppState;
use super::admin_entries::{db, editor, parse};
use super::admin_publishing::holder;
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::access::Permission;
use crate::assist::conflicts::{self, View};
use crate::assist::settings::{self, Settings};
use crate::assist::structure;
use crate::assist::tasks::{self, TaskKind};
use crate::audit::{self, action};
use crate::workspace_db;

pub async fn get_settings(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<Settings>, ApiError> {
    let ws = parse(&ws)?;
    holder(&state, &current, ws, Permission::ManageLlm).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = settings::get(&mut tx).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

pub async fn put_settings(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    JsonBody(req): JsonBody<Settings>,
) -> Result<Json<Settings>, ApiError> {
    let ws = parse(&ws)?;
    holder(&state, &current, ws, Permission::ManageLlm).await?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let before = settings::get(&mut tx).await?;
    settings::set(&mut tx, req).await?;
    if before != req {
        workspace_db::audit_full(
            &mut tx,
            &db,
            Some(ws),
            Some(current.user_id),
            action::ASSIST_SETTINGS_CHANGED,
            &[],
            false,
            Some(&serde_json::json!({ "enabled": req.enabled })),
        )
        .await?;
    }
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(req))
}

pub async fn start_conflicts(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<(StatusCode, Json<View>), ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    editor(&state, &current, ws).await?;
    let out = conflicts::start(&state, ws, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok((StatusCode::ACCEPTED, Json(out)))
}

pub async fn get_conflicts(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<View>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    editor(&state, &current, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = conflicts::view(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

pub async fn start_stale(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<(StatusCode, Json<tasks::View>), ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    editor(&state, &current, ws).await?;
    let out = tasks::start(&state, ws, TaskKind::Stale, &id.to_string())
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok((StatusCode::ACCEPTED, Json(out)))
}

pub async fn get_stale(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<tasks::View>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    editor(&state, &current, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = tasks::view(&mut tx, TaskKind::Stale, &id.to_string())
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

/// 構成案の操作の前の確認（重い権限 viewQuestionLog と再認証。候補のIDの形）。質問文を使うので監査ログに残す。
async fn outline_access(
    state: &AppState,
    current: &crate::auth::session::Current,
    ws: uuid::Uuid,
    id: &str,
) -> Result<(), ApiError> {
    holder(state, current, ws, Permission::ViewQuestionLog).await?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    // 候補のIDは、正規化した質問文のハッシュ（16進の64文字）
    if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ApiError::not_found());
    }
    audit::record(
        &state.pool,
        Some(ws),
        Some(current.user_id),
        action::QUESTION_LOG_VIEWED,
        &[],
    )
    .await?;
    Ok(())
}

pub async fn start_outline(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<(StatusCode, Json<tasks::View>), ApiError> {
    let ws = parse(&ws)?;
    outline_access(&state, &current, ws, &id).await?;
    let out = tasks::start(&state, ws, TaskKind::Outline, &id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok((StatusCode::ACCEPTED, Json(out)))
}

pub async fn get_outline(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<tasks::View>, ApiError> {
    let ws = parse(&ws)?;
    outline_access(&state, &current, ws, &id).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = tasks::view(&mut tx, TaskKind::Outline, &id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

/// 構成の確認の結果（管理APIの応答）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StructureView {
    /// 矛盾の検知と同じ状態（`notStarted`・`running`・`done`・`failed`・`disabled`・`budgetExhausted`）。
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<String>,
    /// 指摘（done のとき）。相手の記事や質問が今も見せてよいものだけ。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestions: Option<Vec<structure::FindingView>>,
    /// 答えられなかった質問（c）を含めるか: `included`・`noPermission`（質問ログの閲覧の権限がない）・
    /// `reauthRequired`（権限はあるが再認証が要る）・`logDisabled`（質問ログが無効）。
    pub questions: &'static str,
    /// 始める前に見せる、トークン数の上限の見積もり（入力・出力）。
    pub estimate: Estimate,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Estimate {
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// 答えられなかった質問を含めるか（重い権限 viewQuestionLog と再認証があり、質問ログが有効なときだけ。2026-11-17）。
async fn structure_questions(
    tx: &mut sqlx::PgConnection,
    access: &crate::access::Access,
    current: &crate::auth::session::Current,
) -> Result<&'static str, ApiError> {
    if !access.has(Permission::ViewQuestionLog) {
        return Ok("noPermission");
    }
    if !current.reauth_fresh {
        return Ok("reauthRequired");
    }
    let enabled: bool = sqlx::query_scalar("SELECT enabled FROM question_log_settings")
        .fetch_one(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
    Ok(if enabled { "included" } else { "logDisabled" })
}

/// いまの下書きの、構成の確認の結果を組み立てる（照合してから見せる。原則3）。
async fn structure_view(
    state: &AppState,
    ws: uuid::Uuid,
    id: uuid::Uuid,
    current: &crate::auth::session::Current,
    access: &crate::access::Access,
) -> Result<StructureView, ApiError> {
    let applied = crate::content::applied_schema(&state.pool, ws)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let db = db(state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let questions = structure_questions(&mut tx, access, current).await?;
    let included = questions == "included";
    let draft = crate::content::entries::draft(&mut tx, &applied.manifest, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let ct = applied
        .manifest
        .content_type(&draft.content_type)
        .ok_or_else(ApiError::not_found)?;
    let docs: Vec<&serde_json::Value> =
        crate::content::publishing::rich_text_fields(ct, &draft.fields)
            .into_iter()
            .map(|(_, v)| v)
            .collect();
    let (input_tokens, output_tokens) = structure::estimate(&docs, included, &draft.locale);
    let view = tasks::view(
        &mut tx,
        TaskKind::Structure,
        &tasks::structure_id(id, included),
    )
    .await?
    .ok_or_else(ApiError::not_found)?;
    let suggestions = match &view.findings {
        Some(f) => {
            Some(structure::show(&mut tx, &applied.manifest, f, included, &draft.locale).await?)
        }
        None => None,
    };
    tx.commit().await.map_err(anyhow::Error::from)?;
    if included {
        // 質問文を使う（見せる）ので、質問ログの閲覧と同じく監査ログに残す
        audit::record(
            &state.pool,
            Some(ws),
            Some(current.user_id),
            action::QUESTION_LOG_VIEWED,
            &[],
        )
        .await?;
    }
    Ok(StructureView {
        state: view.state,
        period: view.period,
        suggestions,
        questions,
        estimate: Estimate {
            input_tokens,
            output_tokens,
        },
    })
}

pub async fn start_structure(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<(StatusCode, Json<StructureView>), ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    let access = editor(&state, &current, ws).await?;
    let included = {
        let db = db(&state, ws).await?;
        let mut tx = workspace_db::begin(&state.pool, &db).await?;
        let q = structure_questions(&mut tx, &access, &current).await?;
        tx.commit().await.map_err(anyhow::Error::from)?;
        q == "included"
    };
    tasks::start(
        &state,
        ws,
        TaskKind::Structure,
        &tasks::structure_id(id, included),
    )
    .await?
    .ok_or_else(ApiError::not_found)?;
    let out = structure_view(&state, ws, id, &current, &access).await?;
    Ok((StatusCode::ACCEPTED, Json(out)))
}

pub async fn get_structure(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<StructureView>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    let access = editor(&state, &current, ws).await?;
    Ok(Json(
        structure_view(&state, ws, id, &current, &access).await?,
    ))
}

/// 校正の提案（管理APIの応答）。`applicable` は、いまの保存済みの下書きで当てられるか（原則4。管理画面は、
/// 当てる前に編集中の本文でも同じ規則で確かめる）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixView {
    pub kind: crate::assist::proofread::FixKind,
    pub from: String,
    pub to: String,
    pub applicable: bool,
}

/// 校正の結果（管理APIの応答）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofreadView {
    /// 矛盾の検知と同じ状態。途中で予算が尽きた・失敗したときも、それまでの提案を返す。
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<String>,
    /// 提案のうち、元の語句がいまの下書きにまだ見つかるものだけ（当てた・書き換えたものは出さない）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestions: Option<Vec<FixView>>,
    /// 結果が、いまの下書きより前のリビジョンについてのものか。
    pub outdated: bool,
    /// 始める前に見せる、トークン数の上限の見積もり（入力・出力）。
    pub estimate: Estimate,
}

/// いまの下書きの、校正の結果を組み立てる（いまの下書きで照合してから見せる。原則3・4）。
async fn proofread_view(
    state: &AppState,
    ws: uuid::Uuid,
    id: uuid::Uuid,
) -> Result<ProofreadView, ApiError> {
    use crate::assist::proofread;
    let applied = crate::content::applied_schema(&state.pool, ws)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let db = db(state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let draft = crate::content::entries::draft(&mut tx, &applied.manifest, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let view = tasks::view(&mut tx, TaskKind::Proofread, &id.to_string())
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    let ct = applied
        .manifest
        .content_type(&draft.content_type)
        .ok_or_else(ApiError::not_found)?;
    let docs: Vec<&serde_json::Value> =
        crate::content::publishing::rich_text_fields(ct, &draft.fields)
            .into_iter()
            .map(|(_, v)| v)
            .collect();
    let (input_tokens, output_tokens) = proofread::estimate(&docs, &draft.locale);
    let suggestions = view.fixes.map(|fixes| {
        fixes
            .into_iter()
            .filter(|f| proofread::found(&docs, &f.from))
            .map(|f| FixView {
                applicable: proofread::applicable(&docs, &f.from),
                kind: f.kind,
                from: f.from,
                to: f.to,
            })
            .collect()
    });
    Ok(ProofreadView {
        state: view.state,
        period: view.period,
        suggestions,
        outdated: view.revision.is_some_and(|r| r != draft.revision),
        estimate: Estimate {
            input_tokens,
            output_tokens,
        },
    })
}

pub async fn start_proofread(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<(StatusCode, Json<ProofreadView>), ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    editor(&state, &current, ws).await?;
    tasks::start(&state, ws, TaskKind::Proofread, &id.to_string())
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(proofread_view(&state, ws, id).await?),
    ))
}

pub async fn get_proofread(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<ProofreadView>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    editor(&state, &current, ws).await?;
    Ok(Json(proofread_view(&state, ws, id).await?))
}
