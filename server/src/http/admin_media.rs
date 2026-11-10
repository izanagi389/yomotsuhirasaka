//! 管理API: メディアとワークスペースの上限。公開: メディアの配信。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | GET | /api/admin/workspaces/{ws}/media | メディアの一覧 |
//! | POST | /api/admin/workspaces/{ws}/media | アップロード（本文はファイルのバイト列そのもの） |
//! | GET | /api/admin/workspaces/{ws}/media/{id} | メディアと、使っている記事 |
//! | DELETE | /api/admin/workspaces/{ws}/media/{id} | 削除（記事で使っていれば 409。`?force=true` で削除する） |
//! | GET | /api/admin/workspaces/{ws}/limits | 容量の上限と使用量 |
//! | PUT | /api/admin/workspaces/{ws}/limits | 容量の上限と、埋め込みの作成の予算の天井を変える（サーバー管理者・再認証。書いた項目だけを変える） |
//! | GET | /media/{ws}/{file} | 配信（認証なし。名前はランダムで推測できない） |
//!
//! - アップロードは管理者・編集者・執筆者とサーバー管理者。削除は管理者・編集者とサーバー管理者（記事の削除と同じ。Q200）。
//! - 画像は中身で形式を判定し、検査してエンコードし直したものだけを保存する（`media::process`）。
//!   処理は別のスレッドで、同時に動かす数と時間に上限を付けて行う。処理のスレッドは止められないので、同時に動かす枠は、
//!   応答を返した後も処理が本当に終わるまで持ち続ける。
//! - 容量の上限を超えるアップロードは 413 にする（INV-MED-05）。
//! - 配信の応答には、スクリプトの実行を禁じるヘッダー（CSPの sandbox、nosniff）を必ず付ける（INV-MED-03）。
//! - 配信はファイルを少しずつ送る（全体をメモリに読まない）。同時に配信する数と、送る時間に上限を付ける（`media::serve`）。
//! - 下書きや非公開の記事で使っているメディアも、URLを知っていれば取得できる（13-security.md「メディア」）。

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use uuid::Uuid;

use super::AppState;
use super::admin_entries::{db, editor, parse, reader};
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::access::{self, Access, Role};
use crate::audit::{self, action};
use crate::auth::session::Current;
use crate::content::applied_schema;
use crate::media::process::{self, Rejected};
use crate::media::serve;
use crate::media::store::{self, MediaInfo, Usage};
use crate::workspace_db;

/// 画像の処理を同時に動かす数（大きな画像のデコードはメモリを多く使うため）。
const CONCURRENT_PROCESSING: usize = 2;

fn processing_slots() -> &'static Arc<Semaphore> {
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SLOTS.get_or_init(|| Arc::new(Semaphore::new(CONCURRENT_PROCESSING)))
}

/// 枠を待って別のスレッドで処理する。枠は `work` が終わるまで持ち続け、時間切れや切断では手放さない
/// （処理のスレッドは止められないので、手放すと画像のデコードがいくつでも重なる）。枠を待つ時間も `timeout` に含める。
async fn run_in_slot<T: Send + 'static>(
    slots: &Arc<Semaphore>,
    timeout: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Slowed> {
    let deadline = tokio::time::Instant::now() + timeout;
    let permit = tokio::time::timeout_at(deadline, slots.clone().acquire_owned())
        .await
        .map_err(|_| Slowed::NoSlot)?
        .map_err(|_| Slowed::Panicked)?;
    let job = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    });
    match tokio::time::timeout_at(deadline, job).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(_)) => Err(Slowed::Panicked),
        Err(_) => Err(Slowed::Timeout),
    }
}

/// [`run_in_slot`] で結果が得られなかった理由。
#[derive(Debug, PartialEq, Eq)]
enum Slowed {
    NoSlot,
    Timeout,
    Panicked,
}

/// 配信のURLの先頭。
pub fn media_base(state: &AppState) -> String {
    match &state.config.media.public_base_url {
        Some(u) => u.trim_end_matches('/').to_owned(),
        None => format!("{}/media", state.config.public_url.trim_end_matches('/')),
    }
}

async fn deleter(state: &AppState, current: &Current, ws: Uuid) -> Result<Access, ApiError> {
    let access = reader(state, current, ws).await?;
    if !(access.server_admin || matches!(access.role, Some(Role::Admin | Role::Editor))) {
        return Err(access::Denied::Forbidden.into());
    }
    Ok(access)
}

/// ワークスペースの容量の上限（バイト）。
async fn quota(state: &AppState, ws: Uuid) -> Result<i64, ApiError> {
    let set: Option<Option<i64>> =
        sqlx::query_scalar("SELECT media_bytes FROM yomi.workspace_limits WHERE workspace_id = $1")
            .bind(ws)
            .fetch_optional(&state.pool)
            .await
            .map_err(anyhow::Error::from)?;
    let default =
        i64::try_from(state.config.media.default_workspace_quota_bytes).unwrap_or(i64::MAX);
    Ok(set.flatten().unwrap_or(default))
}

fn rejected(r: Rejected) -> ApiError {
    match r {
        Rejected::UnsupportedFormat => ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "Only JPEG, PNG, GIF and WebP images are accepted. The type is decided by the file contents.",
        ),
        Rejected::TooManyPixels => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "image_too_large",
            "The image has more pixels than this server accepts.",
        ),
        Rejected::Corrupt => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_image",
            "The file could not be read as an image.",
        ),
    }
}

fn file_too_large() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "file_too_large",
        "The file is larger than this server accepts.",
    )
}

pub async fn upload(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<MediaInfo>), ApiError> {
    let ws = parse(&ws)?;
    let access = editor(&state, &current, ws).await?;
    let body = match body {
        Ok(b) => b,
        Err(e) if e.status() == StatusCode::PAYLOAD_TOO_LARGE => return Err(file_too_large()),
        Err(_) => return Err(ApiError::bad_request("Could not read the uploaded file.")),
    };
    if body.is_empty() {
        return Err(rejected(Rejected::UnsupportedFormat));
    }
    let db = db(&state, ws).await?;
    let quota = quota(&state, ws).await?;
    // 処理の前に、元のファイルの大きさで明らかに超えるものを断る
    {
        let mut tx = workspace_db::begin(&state.pool, &db).await?;
        let used = store::used_bytes(&mut tx).await?;
        tx.commit().await.map_err(anyhow::Error::from)?;
        if used >= quota {
            return Err(quota_exceeded());
        }
    }

    let max_pixels = state.config.media.max_pixels;
    let timeout = Duration::from_secs(u64::from(state.config.media.processing_timeout_seconds));
    let job = run_in_slot(processing_slots(), timeout, move || {
        process::process(&body, max_pixels)
    });
    let processed = match job.await {
        Ok(Ok(p)) => p,
        Ok(Err(r)) => return Err(rejected(r)),
        Err(Slowed::Panicked) => return Err(rejected(Rejected::Corrupt)),
        Err(Slowed::NoSlot) => {
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "media_processing_busy",
                "Too many images are being processed. Try again later.",
            ));
        }
        // 時間切れ: 処理のスレッドは止められないが、上限の画素数で抑えてあり、終わるまで枠を持ち続ける
        Err(Slowed::Timeout) => {
            return Err(ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "image_processing_timeout",
                "The image took too long to process.",
            ));
        }
    };
    if processed.bytes.len() as u64 > state.config.media.max_file_bytes {
        return Err(file_too_large());
    }

    let root = &state.config.media.root;
    let file = store::write_file(root, ws, &processed).await?;
    let base = media_base(&state);
    let id = Uuid::new_v4();
    let saved = async {
        let mut tx = workspace_db::begin(&state.pool, &db).await?;
        store::lock_uploads(&mut tx).await?;
        let used = store::used_bytes(&mut tx).await?;
        if used.saturating_add(processed.bytes.len() as i64) > quota {
            return Ok(None);
        }
        let info =
            store::insert(&mut tx, &base, ws, id, &file, &processed, current.user_id).await?;
        if access.server_admin {
            workspace_db::audit(
                &mut tx,
                &db,
                Some(ws),
                Some(current.user_id),
                action::MEDIA_UPLOADED,
                &[id],
            )
            .await?;
        }
        tx.commit().await?;
        anyhow::Ok(Some(info))
    }
    .await;
    let info = match saved {
        Ok(Some(info)) => info,
        Ok(None) => {
            store::remove_file(root, ws, &file).await?;
            return Err(quota_exceeded());
        }
        Err(e) => {
            let _ = store::remove_file(root, ws, &file).await;
            return Err(e.into());
        }
    };
    Ok((StatusCode::CREATED, Json(info)))
}

fn quota_exceeded() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "media_quota_exceeded",
        "This workspace has reached its storage limit for media.",
    )
}

pub async fn list(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<Vec<MediaInfo>>, ApiError> {
    let ws = parse(&ws)?;
    reader(&state, &current, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = store::list(&mut tx, &media_base(&state), ws).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaDetail {
    #[serde(flatten)]
    info: MediaInfo,
    /// 使っている記事（スキーマが未デプロイなら空）。
    usage: Vec<Usage>,
}

async fn usage_of(
    state: &AppState,
    tx: &mut sqlx::PgConnection,
    ws: Uuid,
    id: Uuid,
) -> Result<Vec<Usage>, ApiError> {
    Ok(match applied_schema(&state.pool, ws).await? {
        Some(applied) => store::usage(tx, &applied.manifest, id).await?,
        None => Vec::new(),
    })
}

pub async fn get(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<MediaDetail>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    reader(&state, &current, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let info = store::get(&mut tx, &media_base(&state), ws, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let usage = usage_of(&state, &mut tx, ws, id).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(MediaDetail { info, usage }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteQuery {
    #[serde(default)]
    force: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InUse {
    error: &'static str,
    message: &'static str,
    usage: Vec<Usage>,
}

pub async fn delete(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
    Query(q): Query<DeleteQuery>,
) -> Result<Response, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    let access = deleter(&state, &current, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    if store::get(&mut tx, "", ws, id).await?.is_none() {
        return Err(ApiError::not_found());
    }
    // 記事で使っていれば、確認を求める（`force` で削除する）
    let usage = usage_of(&state, &mut tx, ws, id).await?;
    if !usage.is_empty() && !q.force {
        return Ok((
            StatusCode::CONFLICT,
            Json(InUse {
                error: "media_in_use",
                message: "This media is used by entries. Images in those entries will no longer be shown. Send `force=true` to delete it anyway.",
                usage,
            }),
        )
            .into_response());
    }
    let file = store::mark_deleted(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if access.server_admin {
        workspace_db::audit(
            &mut tx,
            &db,
            Some(ws),
            Some(current.user_id),
            action::MEDIA_DELETED,
            &[id],
        )
        .await?;
    }
    tx.commit().await.map_err(anyhow::Error::from)?;
    store::remove_file(&state.config.media.root, ws, &file).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    media_bytes: i64,
    media_bytes_used: i64,
    /// LLM の接続設定の上書きを許しているか（M4）。
    llm_override: bool,
}

async fn limits_of(state: &AppState, ws: Uuid) -> Result<Limits, ApiError> {
    let db = db(state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let used = store::used_bytes(&mut tx).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Limits {
        media_bytes: quota(state, ws).await?,
        media_bytes_used: used,
        llm_override: crate::workspace_llm::allowed(&state.pool, ws).await?,
    })
}

pub async fn get_limits(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<Limits>, ApiError> {
    let ws = parse(&ws)?;
    reader(&state, &current, ws).await?;
    Ok(Json(limits_of(&state, ws).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetLimits {
    /// メディアの容量の上限（バイト）。null で設定の既定値に戻す。項目がなければ変えない（天井だけを変えるときのため）。
    #[serde(default, deserialize_with = "present")]
    media_bytes: Option<Option<i64>>,
    /// 埋め込みの作成の予算の天井（トークン数、UTCの暦の月・日。`budget.rs`）。null で設定の既定値に戻す。項目がなければ変えない。
    #[serde(default, deserialize_with = "present")]
    embedding_tokens_per_month: Option<Option<i64>>,
    #[serde(default, deserialize_with = "present")]
    embedding_tokens_per_day: Option<Option<i64>>,
    /// 訪問者向けの回答の予算の天井（トークン数、入力・出力別、UTCの暦の月・日。M4）。null と「項目なし」は埋め込みと同じ。
    #[serde(default, deserialize_with = "present")]
    answer_input_tokens_per_month: Option<Option<i64>>,
    #[serde(default, deserialize_with = "present")]
    answer_input_tokens_per_day: Option<Option<i64>>,
    #[serde(default, deserialize_with = "present")]
    answer_output_tokens_per_month: Option<Option<i64>>,
    #[serde(default, deserialize_with = "present")]
    answer_output_tokens_per_day: Option<Option<i64>>,
    /// 編集者向けの支援の予算の天井（トークン数、入力・出力別、UTCの暦の月・日。M4）。null と「項目なし」は埋め込みと同じ。
    #[serde(default, deserialize_with = "present")]
    assist_input_tokens_per_month: Option<Option<i64>>,
    #[serde(default, deserialize_with = "present")]
    assist_input_tokens_per_day: Option<Option<i64>>,
    #[serde(default, deserialize_with = "present")]
    assist_output_tokens_per_month: Option<Option<i64>>,
    #[serde(default, deserialize_with = "present")]
    assist_output_tokens_per_day: Option<Option<i64>>,
    /// ワークスペースに LLM の接続設定の上書きを許すか（M4、Q26）。false にしたら、保存してある上書き（キーを含む）を消す。
    #[serde(default)]
    llm_override: Option<bool>,
}

/// 項目が「ない」ことと「null」を区別する（ない: None、null: Some(None)）。
fn present<'de, D>(d: D) -> Result<Option<Option<i64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<i64>::deserialize(d).map(Some)
}

pub async fn set_limits(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    JsonBody(req): JsonBody<SetLimits>,
) -> Result<Json<Limits>, ApiError> {
    let ws = parse(&ws)?;
    access::server_admin(&current)?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    // ワークスペースがあることを確かめる（なければ 404）
    reader(&state, &current, ws).await?;
    if req.media_bytes.flatten().is_some_and(|b| b < 0) {
        return Err(ApiError::bad_request("`mediaBytes` must not be negative."));
    }
    if [req.embedding_tokens_per_month, req.embedding_tokens_per_day]
        .iter()
        .any(|v| v.flatten().is_some_and(|t| t < 0))
    {
        return Err(ApiError::bad_request(
            "`embeddingTokensPerMonth` and `embeddingTokensPerDay` must not be negative.",
        ));
    }
    // 回答の予算の天井（決まった4つの列。SQL は固定の文）
    let answer = [
        (
            "UPDATE yomi.workspace_limits SET answer_input_tokens_per_month = $2 WHERE workspace_id = $1",
            req.answer_input_tokens_per_month,
        ),
        (
            "UPDATE yomi.workspace_limits SET answer_input_tokens_per_day = $2 WHERE workspace_id = $1",
            req.answer_input_tokens_per_day,
        ),
        (
            "UPDATE yomi.workspace_limits SET answer_output_tokens_per_month = $2 WHERE workspace_id = $1",
            req.answer_output_tokens_per_month,
        ),
        (
            "UPDATE yomi.workspace_limits SET answer_output_tokens_per_day = $2 WHERE workspace_id = $1",
            req.answer_output_tokens_per_day,
        ),
    ];
    if answer
        .iter()
        .any(|(_, v)| v.flatten().is_some_and(|t| t < 0))
    {
        return Err(ApiError::bad_request(
            "The answer token limits must not be negative.",
        ));
    }
    // 支援の予算の天井（決まった4つの列。SQL は固定の文）
    let assist = [
        (
            "UPDATE yomi.workspace_limits SET assist_input_tokens_per_month = $2 WHERE workspace_id = $1",
            req.assist_input_tokens_per_month,
        ),
        (
            "UPDATE yomi.workspace_limits SET assist_input_tokens_per_day = $2 WHERE workspace_id = $1",
            req.assist_input_tokens_per_day,
        ),
        (
            "UPDATE yomi.workspace_limits SET assist_output_tokens_per_month = $2 WHERE workspace_id = $1",
            req.assist_output_tokens_per_month,
        ),
        (
            "UPDATE yomi.workspace_limits SET assist_output_tokens_per_day = $2 WHERE workspace_id = $1",
            req.assist_output_tokens_per_day,
        ),
    ];
    if assist
        .iter()
        .any(|(_, v)| v.flatten().is_some_and(|t| t < 0))
    {
        return Err(ApiError::bad_request(
            "The assist token limits must not be negative.",
        ));
    }
    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    sqlx::query(
        "INSERT INTO yomi.workspace_limits (workspace_id, updated_by) VALUES ($1, $2)
         ON CONFLICT (workspace_id) DO UPDATE SET updated_by = $2, updated_at = now()",
    )
    .bind(ws)
    .bind(current.user_id)
    .execute(&mut *tx)
    .await
    .map_err(anyhow::Error::from)?;
    if let Some(allow) = req.llm_override {
        sqlx::query("UPDATE yomi.workspace_limits SET llm_override = $2 WHERE workspace_id = $1")
            .bind(ws)
            .bind(allow)
            .execute(&mut *tx)
            .await
            .map_err(anyhow::Error::from)?;
        if !allow {
            crate::workspace_llm::remove(&mut *tx, ws).await?;
        }
    }
    if let Some(v) = req.media_bytes {
        sqlx::query("UPDATE yomi.workspace_limits SET media_bytes = $2 WHERE workspace_id = $1")
            .bind(ws)
            .bind(v)
            .execute(&mut *tx)
            .await
            .map_err(anyhow::Error::from)?;
    }
    if let Some(v) = req.embedding_tokens_per_month {
        sqlx::query("UPDATE yomi.workspace_limits SET embedding_tokens_per_month = $2 WHERE workspace_id = $1")
            .bind(ws)
            .bind(v)
            .execute(&mut *tx)
            .await
            .map_err(anyhow::Error::from)?;
    }
    if let Some(v) = req.embedding_tokens_per_day {
        sqlx::query(
            "UPDATE yomi.workspace_limits SET embedding_tokens_per_day = $2 WHERE workspace_id = $1",
        )
        .bind(ws)
        .bind(v)
        .execute(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
    }
    for (sql, value) in answer.into_iter().chain(assist) {
        if let Some(v) = value {
            sqlx::query(sql)
                .bind(ws)
                .bind(v)
                .execute(&mut *tx)
                .await
                .map_err(anyhow::Error::from)?;
        }
    }
    if answer.iter().any(|(_, v)| v.is_some()) {
        crate::budget::resumed(&mut *tx, ws, crate::budget::Kind::VisitorAnswers).await?;
    }
    if assist.iter().any(|(_, v)| v.is_some()) {
        crate::budget::resumed(&mut *tx, ws, crate::budget::Kind::EditorAssist).await?;
    }
    let embedding =
        req.embedding_tokens_per_month.is_some() || req.embedding_tokens_per_day.is_some();
    if embedding {
        crate::budget::resumed(&mut *tx, ws, crate::budget::Kind::Embedding).await?;
    }
    audit::record(
        &mut *tx,
        Some(ws),
        Some(current.user_id),
        action::WORKSPACE_LIMITS_CHANGED,
        &[ws],
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    if embedding {
        // 予算で止まっていた検索用データの作成を、すぐにやり直す
        super::admin_publishing::schedule_search(&state, ws).await;
    }
    Ok(Json(limits_of(&state, ws).await?))
}

/// メディアの応答のヘッダー（INV-MED-03）。セキュリティヘッダーの共通の値より先に付ける。
const MEDIA_HEADERS: &[(&str, &str)] = &[
    ("content-security-policy", "default-src 'none'; sandbox"),
    ("x-content-type-options", "nosniff"),
    // 公開サイトから画像として読み込めるようにする
    ("cross-origin-resource-policy", "cross-origin"),
    // 削除したら取得できなくなるよう、長くはキャッシュさせない
    ("cache-control", "public, max-age=3600"),
    ("content-disposition", "inline"),
];

pub async fn serve(
    State(state): State<AppState>,
    Path((ws, file)): Path<(String, String)>,
) -> Response {
    let Some((path, format)) = store::served_path(&state.config.media.root, &ws, &file) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(permit) = state.media_serves.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let opened = async {
        let file = tokio::fs::File::open(&path).await?;
        let meta = file.metadata().await?;
        std::io::Result::Ok((file, meta))
    };
    let (file, len) = match opened.await {
        Ok((file, meta)) if meta.is_file() => (file, meta.len()),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let mut res = serve::file_body(file, len, permit, serve::SendLimits::DEFAULT).into_response();
    let headers = res.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(format.content_type()),
    );
    for (name, value) in MEDIA_HEADERS {
        headers.insert(*name, HeaderValue::from_static(value));
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_processing_slot_is_held_until_the_work_really_ends() {
        let slots = Arc::new(Semaphore::new(1));
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let timed_out = run_in_slot(&slots, Duration::from_millis(30), move || {
            let _ = wait.recv();
        })
        .await;
        assert_eq!(timed_out, Err(Slowed::Timeout));
        // 応答は返っても、処理が終わるまで枠は空かない
        assert_eq!(slots.available_permits(), 0);
        let queued = run_in_slot(&slots, Duration::from_millis(30), || ()).await;
        assert_eq!(queued, Err(Slowed::NoSlot));
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), slots.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }

    #[tokio::test]
    async fn a_processing_slot_comes_back_after_success() {
        let slots = Arc::new(Semaphore::new(1));
        assert_eq!(
            run_in_slot(&slots, Duration::from_secs(5), || 7).await,
            Ok(7)
        );
        tokio::time::timeout(Duration::from_secs(5), slots.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
}
