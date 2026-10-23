//! 管理API: 公開・非公開・削除、版、URLの変更履歴。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | POST | /api/admin/workspaces/{ws}/entries/{id}/publish | 公開する（`{revision, correction?}`） |
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/publish-check | 公開の確認画面の情報（変わったセクション、代替テキストのない画像など） |
//! | POST | /api/admin/workspaces/{ws}/entries/{id}/unpublish | 非公開にする |
//! | DELETE | /api/admin/workspaces/{ws}/entries/{id} | 削除する |
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/publish-settings | 公開設定のスイッチ |
//! | PATCH | /api/admin/workspaces/{ws}/entries/{id}/publish-settings | 公開設定のスイッチを変える（送った項目だけ） |
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/versions | 版の一覧（古い順） |
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/versions/{vid} | 版（抹消した版は本文なし） |
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/url-history | 公開URLの変更履歴 |
//! | GET | /api/admin/workspaces/{ws}/entries/{id}/versions/{vid}/erase-candidates | 抹消と一緒に削除できるメディアと、他のロケールの記事 |
//! | POST | /api/admin/workspaces/{ws}/entries/{id}/versions/{vid}/erase | 版の本文を抹消する（`{deleteMediaIds}`） |
//!
//! - 公開・非公開・削除は、管理者・編集者とサーバー管理者だけ（執筆者はできない。Q77, Q200、INV-PERM-01）。
//! - 訂正としての公開は、重い権限「訂正としての公開と確認済みの記録」を持つ人（管理者・編集者の既定と、個人への付与。Q76、
//!   INV-PERM-03）。付与された執筆者も、通常の公開はできない。
//! - 公開は移行待ちの間は受け付けない（INV-SCH-04）。非公開と削除は、記事を急いで取り下げられるよう、移行待ちの間も受け付ける。
//! - 訂正としての公開は、誰が行っても監査ログに残す（11-permissions-and-audit.md）。それ以外は、所属していないサーバー管理者が
//!   行ったときに残す（Q122）。
//! - 版の本文の抹消は、管理者とサーバー管理者だけで、再認証を求め、必ず監査ログに残す（Q134）。現在公開中の版は抹消できない。
//! - 公開の確認（publish-check）は、公開と同じ判定（Rustの sections.rs）を書き込まずに行う。変わったセクションは、訂正として
//!   公開したときに版に記録するものと同じ（Q68）。読むだけなので、下書きを読める人なら使える。
//! - 版とURLの変更履歴を読むのは、下書きと同じく所属者全員とサーバー管理者。
//! - 公開設定のスイッチ（Q184）は、サイトの方針に関わる重い権限（管理者の既定と、個人への付与。Q76、INV-PERM-03）を
//!   持つ人だけが変えられる。読むのは所属者全員とサーバー管理者。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use uuid::Uuid;

use super::AppState;
use super::admin_entries::{db, invalid, parse, readable, reader, revision_conflict, writable};
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::access::{self, Access, Permission, Role};
use crate::audit::{self, action};
use crate::auth::session::Current;
use crate::content::corrections;
use crate::content::hooks;
use crate::content::publishing::{
    self, PublishCheck, Published, Refused, Scripts, UrlChange, VersionDetail, VersionInfo,
};
use crate::content::settings::{self, PublishSettings};
use crate::script::Host;
use crate::webhooks::{self, EntryEvent, EntryRef};
use crate::workspace_db;

/// 重い権限を持つ人なら通す（ロールの既定か、個人への付与。リクエストのたびに照合する）。
pub(super) async fn holder(
    state: &AppState,
    current: &Current,
    ws: Uuid,
    permission: Permission,
) -> Result<Access, ApiError> {
    let access = reader(state, current, ws).await?;
    if !access.has(permission) {
        return Err(access::Denied::Forbidden.into());
    }
    Ok(access)
}

pub(super) async fn publisher(
    state: &AppState,
    current: &Current,
    ws: Uuid,
) -> Result<Access, ApiError> {
    let access = reader(state, current, ws).await?;
    if !(access.server_admin || matches!(access.role, Some(Role::Admin | Role::Editor))) {
        return Err(access::Denied::Forbidden.into());
    }
    Ok(access)
}

fn refused(r: Refused) -> ApiError {
    match r {
        Refused::NotFound => ApiError::not_found(),
        Refused::RevisionConflict => revision_conflict(),
        Refused::Invalid(problems) => invalid(problems),
        Refused::AltTextMissing(places) => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "alt_text_missing",
            format!(
                "Every image needs alternative text or the decorative mark before publishing: {}",
                places.join("; ")
            ),
        ),
        Refused::UrlTaken => url_taken(),
        Refused::NothingToCorrect => ApiError::new(
            StatusCode::CONFLICT,
            "nothing_to_correct",
            "This entry has never been published, so it cannot be published as a correction.",
        ),
        // 関数の例外の文は記事の内容を含みうるので、種類だけを返す（原則7）
        Refused::UrlFunctionFailed(f) => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "url_function_failed",
            format!(
                "The public URL could not be made because {}. Ask the developer of the template to fix the URL function.",
                f.describe()
            ),
        ),
        // 理由の文はテンプレートの開発者が書いたもの（編集者に見せる）
        Refused::HookRejected(reasons) => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "publish_rejected_by_hook",
            format!(
                "The template's beforePublish hook stopped publishing: {}",
                reasons.join("; ")
            ),
        ),
        Refused::ConversionFailed(f) => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "conversion_failed",
            format!(
                "The body could not be converted because {}. Ask the developer of the template to fix the custom node's conversion function.",
                f.describe()
            ),
        ),
        Refused::HookFailed(f) => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "hook_failed",
            format!(
                "The template's beforePublish hook failed because {}. Ask the developer of the template to fix it.",
                f.describe()
            ),
        ),
    }
}

fn url_taken() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "url_taken",
        "Another published entry already uses this URL.",
    )
}

/// 公開URLの一意の索引の違反か（同時に同じURLで公開したとき）。
fn is_url_conflict(e: &anyhow::Error) -> bool {
    e.downcast_ref::<sqlx::Error>()
        .and_then(|e| e.as_database_error())
        .is_some_and(|d| d.constraint() == Some("entries_published_url"))
}

/// 記事のイベントのWebhookを送る（操作を確定した後に呼ぶ。Q146、INV-WH-06）。
/// 送信の準備に失敗しても、確定した操作は取り消せないので、記録して成功を返す。
async fn notify_webhooks(
    state: &AppState,
    ws: Uuid,
    events: &[EntryEvent],
    entry: Option<EntryRef>,
) {
    let Some(entry) = entry else { return };
    if let Err(e) = webhooks::emit(&state.pool, &state.keys, ws, events, &entry).await {
        tracing::error!(workspace_id = %ws, entry_id = %entry.id, "cannot queue webhooks: {e:#}");
    }
}

/// 公開後・非公開の後のフックと、この記事に依存する記事のURLの再計算を積む（操作を確定した後に呼ぶ）。
/// 積めなくても操作は確定しているので、記録して続ける。
pub(super) async fn schedule_hooks(
    state: &AppState,
    ws: Uuid,
    id: Uuid,
    event: EntryEvent,
    version_id: Option<Uuid>,
) {
    if let Err(e) = hooks::schedule_after(&state.pool, &state.keys, ws, id, event, version_id).await
    {
        tracing::error!(workspace_id = %ws, entry_id = %id, "cannot queue hooks: {e:#}");
    }
    if let Err(e) =
        crate::content::urls::schedule_dependents(&state.pool, &state.keys, ws, id).await
    {
        tracing::error!(workspace_id = %ws, entry_id = %id, "cannot queue URL recomputation: {e:#}");
    }
}

/// 公開のイベント: 公開・更新・訂正のどれか1つと、URLが変わったときは URLの変更。
fn publish_events(p: &Published) -> Vec<EntryEvent> {
    let mut events = vec![if p.is_correction {
        EntryEvent::Corrected
    } else if p.was_published {
        EntryEvent::Updated
    } else {
        EntryEvent::Published
    }];
    if p.previous_url.is_some() {
        events.push(EntryEvent::UrlChanged);
    }
    events
}

async fn audit_if(
    state: &AppState,
    always: bool,
    access: &Access,
    current: &Current,
    ws: Uuid,
    act: &str,
    id: Uuid,
) -> Result<(), ApiError> {
    if always || access.server_admin {
        audit::record(&state.pool, Some(ws), Some(current.user_id), act, &[id]).await?;
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Publish {
    /// 公開する下書きのリビジョン番号（読んだときの値）。
    revision: i64,
    /// 訂正として公開するか。
    #[serde(default)]
    correction: bool,
    /// 訂正の説明（定型文の選択と自由記述。訂正として公開するときだけ。後から書いてもよい）。
    #[serde(default)]
    correction_note: Option<corrections::NoteInput>,
}

pub async fn publish(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
    JsonBody(req): JsonBody<Publish>,
) -> Result<Json<Published>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    let access = if req.correction {
        holder(&state, &current, ws, Permission::Corrections).await?
    } else {
        publisher(&state, &current, ws).await?
    };
    if req.correction_note.is_some() && !req.correction {
        return Err(invalid(vec![
            "correctionNote: only when publishing as a correction".to_owned(),
        ]));
    }
    let applied = writable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    // 説明は公開と同じトランザクションで保存する（定型文はその時点の文を写す）
    let note = match &req.correction_note {
        Some(input) => match corrections::resolve_note(&mut tx, input).await? {
            Some(text) if corrections::valid_note(&text) => Some(text),
            Some(_) => {
                return Err(invalid(vec![format!(
                    "correctionNote: at most {} characters without control characters",
                    corrections::MAX_NOTE_CHARS
                )]));
            }
            None => {
                return Err(invalid(vec![
                    "correctionNote.templateId: unknown template".to_owned(),
                ]));
            }
        },
        None => None,
    };
    let scripts = Scripts {
        engine: &state.scripts,
        host: Host::new(state.pool.clone(), db.clone()),
    };
    let result = publishing::publish(
        &mut tx,
        &applied.manifest,
        &scripts,
        applied.version,
        id,
        req.revision,
        req.correction,
        current.user_id,
    )
    .await;
    let published = match result {
        Ok(Ok(p)) => p,
        Ok(Err(r)) => return Err(refused(r)),
        Err(e) if is_url_conflict(&e) => return Err(url_taken()),
        Err(e) => return Err(e.into()),
    };
    // 開いているレビュー依頼は「公開済み」として閉じる（承認という段階は作らない。Q77）
    crate::content::reviews::close(
        &mut tx,
        id,
        crate::content::reviews::State::Published,
        current.user_id,
        None,
    )
    .await?;
    if let Some(text) = note.as_deref().filter(|t| !t.is_empty()) {
        corrections::save_note(
            &mut tx,
            id,
            published.version_id,
            text,
            Some(current.user_id),
        )
        .await?;
    }
    let entry = webhooks::entry_ref(&mut tx, id).await?;
    match tx.commit().await {
        Ok(()) => {}
        Err(e) => {
            let e = anyhow::Error::from(e);
            return Err(if is_url_conflict(&e) {
                url_taken()
            } else {
                e.into()
            });
        }
    }
    let act = if req.correction {
        action::ENTRY_PUBLISHED_AS_CORRECTION
    } else {
        action::ENTRY_PUBLISHED
    };
    let events = publish_events(&published);
    notify_webhooks(&state, ws, &events, entry).await;
    schedule_hooks(&state, ws, id, events[0], Some(published.version_id)).await;
    audit_if(&state, req.correction, &access, &current, ws, act, id).await?;
    Ok(Json(published))
}

pub async fn unpublish(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    let access = publisher(&state, &current, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let was_published = publishing::unpublish(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let entry = webhooks::entry_ref(&mut tx, id).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    if was_published {
        notify_webhooks(&state, ws, &[EntryEvent::Unpublished], entry).await;
        schedule_hooks(&state, ws, id, EntryEvent::Unpublished, None).await;
    }
    audit_if(
        &state,
        false,
        &access,
        &current,
        ws,
        action::ENTRY_UNPUBLISHED,
        id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    let access = publisher(&state, &current, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    if !publishing::delete(&mut tx, id).await? {
        return Err(ApiError::not_found());
    }
    let entry = webhooks::entry_ref(&mut tx, id).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    notify_webhooks(&state, ws, &[EntryEvent::Deleted], entry).await;
    schedule_hooks(&state, ws, id, EntryEvent::Deleted, None).await;
    audit_if(
        &state,
        false,
        &access,
        &current,
        ws,
        action::ENTRY_DELETED,
        id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn publish_check(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<PublishCheck>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    reader(&state, &current, ws).await?;
    let applied = readable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let scripts = Scripts {
        engine: &state.scripts,
        host: Host::new(state.pool.clone(), db.clone()),
    };
    let out = publishing::check_before_publish(&mut tx, &applied.manifest, &scripts, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

pub async fn versions(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<Vec<VersionInfo>>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    reader(&state, &current, ws).await?;
    readable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = publishing::versions(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

pub async fn version(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id, vid)): Path<(String, String, String)>,
) -> Result<Json<VersionDetail>, ApiError> {
    let (ws, id, vid) = (parse(&ws)?, parse(&id)?, parse(&vid)?);
    reader(&state, &current, ws).await?;
    readable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = publishing::version(&mut tx, id, vid)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

pub async fn url_history(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<Vec<UrlChange>>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    reader(&state, &current, ws).await?;
    readable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = publishing::url_history(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

/// 抹消は管理者とサーバー管理者だけ（重い操作のため再認証も求める）。
async fn eraser(state: &AppState, current: &Current, ws: Uuid) -> Result<Access, ApiError> {
    let access = reader(state, current, ws).await?;
    if !(access.server_admin || access.role == Some(Role::Admin)) {
        return Err(access::Denied::Forbidden.into());
    }
    Ok(access)
}

pub async fn erase_candidates(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id, vid)): Path<(String, String, String)>,
) -> Result<Json<crate::content::erase::Candidates>, ApiError> {
    let (ws, id, vid) = (parse(&ws)?, parse(&id)?, parse(&vid)?);
    eraser(&state, &current, ws).await?;
    let applied = readable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = crate::content::erase::candidates(&mut tx, &applied.manifest, id, vid)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Erase {
    /// 一緒に削除するメディア（どこかで使われているものは削除しない）。
    #[serde(default)]
    delete_media_ids: Vec<Uuid>,
}

pub async fn erase(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id, vid)): Path<(String, String, String)>,
    JsonBody(req): JsonBody<Erase>,
) -> Result<Json<crate::content::erase::Erased>, ApiError> {
    use crate::content::erase::{self, Refused as EraseRefused};
    let (ws, id, vid) = (parse(&ws)?, parse(&id)?, parse(&vid)?);
    eraser(&state, &current, ws).await?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    if req.delete_media_ids.len() > 1000 {
        return Err(ApiError::bad_request("Too many media IDs."));
    }
    let applied = readable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let erased = match erase::erase(&mut tx, &applied.manifest, id, vid, &req.delete_media_ids)
        .await?
    {
        Ok(e) => e,
        Err(EraseRefused::NotFound) => return Err(ApiError::not_found()),
        Err(EraseRefused::Published) => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "version_is_published",
                "The published version cannot be erased. Publish a corrected version or unpublish the entry first.",
            ));
        }
    };
    let entry = webhooks::entry_ref(&mut tx, id).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    notify_webhooks(&state, ws, &[EntryEvent::VersionErased], entry).await;
    for file in &erased.deleted_files {
        crate::media::store::remove_file(&state.config.media.root, ws, file).await?;
    }
    let mut targets = vec![id, vid];
    targets.extend(&erased.deleted_media_ids);
    audit::record(
        &state.pool,
        Some(ws),
        Some(current.user_id),
        action::VERSION_ERASED,
        &targets,
    )
    .await?;
    Ok(Json(erased))
}

pub async fn get_settings(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<PublishSettings>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    reader(&state, &current, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = settings::get(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

pub async fn change_settings(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
    JsonBody(req): JsonBody<settings::Change>,
) -> Result<Json<PublishSettings>, ApiError> {
    let (ws, id) = (parse(&ws)?, parse(&id)?);
    let access = holder(&state, &current, ws, Permission::PolicySwitches).await?;
    if req.is_empty() {
        return Err(ApiError::bad_request(
            "Send at least one of `useForChatbot`, `includeInAiMaterials` and `optOutOfAiTraining`.",
        ));
    }
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let (after, changed) = settings::change(&mut tx, id, req)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let entry = webhooks::entry_ref(&mut tx, id).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    if changed {
        notify_webhooks(&state, ws, &[EntryEvent::SettingsChanged], entry).await;
        audit_if(
            &state,
            false,
            &access,
            &current,
            ws,
            action::ENTRY_SETTINGS_CHANGED,
            id,
        )
        .await?;
    }
    Ok(Json(after))
}
