//! 管理API: 既存のブログからの取り込み（03-content-and-editing.md「既存ブログからの取り込み」、Q74）。取り込みのCLIが使う。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | POST | /api/admin/workspaces/{ws}/imports | 記事をまとめて取り込む（`{contentType?, entries: [...]}`） |
//!
//! - 取り込んだ記事は公開する（元のサイトで公開されていた記事なので、リダイレクトの行き先が要る）。
//!   そのため、公開できる人（管理者・編集者とサーバー管理者）だけが使える。移行待ちの間は受け付けない。
//! - 1つのトランザクションで行い、1件でも受け付けられなければ何も書かない（INV-IMP-01）。
//! - 元のURLから新しいURLへの対応をURLの変更履歴に残す（INV-IMP-02）。
//! - 監査ログに、取り込んだ記事のIDを残す。確定の後に、記事ごとに公開のWebhookを送る。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use super::AppState;
use super::admin_entries::{db, invalid, parse, reader, writable};
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::access::{self, Role};
use crate::audit::{self, action};
use crate::content::entries;
use crate::content::import::{self, Failed, Imported, Item};
use crate::content::publishing::{Refused, Scripts};
use crate::script::Host;
use crate::webhooks::{self, EntryEvent};
use crate::workspace_db;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportRequest {
    /// 省略時は、テンプレートにコンテンツタイプが1つだけならそれ。
    content_type: Option<String>,
    entries: Vec<Item>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    entries: Vec<Imported>,
}

pub async fn import(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    JsonBody(req): JsonBody<ImportRequest>,
) -> Result<(StatusCode, Json<ImportResult>), ApiError> {
    let ws = parse(&ws)?;
    let access = reader(&state, &current, ws).await?;
    if !(access.server_admin || matches!(access.role, Some(Role::Admin | Role::Editor))) {
        return Err(access::Denied::Forbidden.into());
    }
    let applied = writable(&state, ws).await?;
    let manifest = &applied.manifest;
    let ct = match req.content_type.as_deref() {
        Some(name) => manifest.content_type(name),
        None if manifest.content_types.len() == 1 => manifest.content_types.first(),
        None => {
            return Err(invalid(vec![
                "contentType: required when the template has several content types".to_owned(),
            ]));
        }
    }
    .ok_or_else(|| invalid(vec!["contentType: unknown content type".to_owned()]))?;
    let plan = import::plan(manifest, ct, &req.entries).map_err(invalid)?;

    let db = db(&state, ws).await?;
    let _hook_slot = state.hook_slot().await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let limit = state.config.limits.max_entries_per_workspace;
    if !entries::room_for(&mut tx, limit, req.entries.len()).await? {
        return Err(super::admin_entries::entry_limit_reached());
    }
    let scripts = Scripts {
        engine: &state.scripts,
        host: Host::new(state.pool.clone(), db.clone()),
    };
    let imported = match import::apply(
        &mut tx,
        manifest,
        &scripts,
        applied.version,
        ct,
        plan,
        current.user_id,
    )
    .await
    {
        Ok(Ok(imported)) => imported,
        Ok(Err(Failed::SlugTaken(key))) => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "slug_taken",
                format!(
                    "{key}: another entry in this locale already uses this slug. Nothing was imported."
                ),
            ));
        }
        Ok(Err(Failed::Refused(key, r))) => {
            let why = match r {
                Refused::Invalid(p) => p.join("; "),
                Refused::AltTextMissing(places) => {
                    format!(
                        "images need alternative text or the decorative mark: {}",
                        places.join("; ")
                    )
                }
                Refused::UrlTaken => "another published entry already uses this URL".to_owned(),
                Refused::UrlFunctionFailed(f) => {
                    format!("the public URL could not be made because {}", f.describe())
                }
                Refused::HookRejected(reasons) => {
                    format!("the beforePublish hook stopped it: {}", reasons.join("; "))
                }
                Refused::ConversionFailed(f) => {
                    format!("the body could not be converted because {}", f.describe())
                }
                Refused::HookFailed(f) => {
                    format!("the beforePublish hook failed because {}", f.describe())
                }
                _ => "it cannot be published".to_owned(),
            };
            return Err(invalid(vec![format!(
                "{key}: {why}. Nothing was imported."
            )]));
        }
        Err(e) => return Err(e.into()),
    };
    let mut refs = Vec::with_capacity(imported.len());
    for i in &imported {
        refs.push(webhooks::entry_ref(&mut tx, i.id).await?);
    }
    tx.commit().await.map_err(anyhow::Error::from)?;

    let ids: Vec<uuid::Uuid> = imported.iter().map(|i| i.id).collect();
    audit::record(
        &state.pool,
        Some(ws),
        Some(current.user_id),
        action::ENTRIES_IMPORTED,
        &ids,
    )
    .await?;
    for entry in refs.into_iter().flatten() {
        if let Err(e) = webhooks::emit(
            &state.pool,
            &state.keys,
            ws,
            &[EntryEvent::Published],
            &entry,
        )
        .await
        {
            tracing::error!(workspace_id = %ws, entry_id = %entry.id, "cannot queue webhooks: {e:#}");
        }
        let version = imported
            .iter()
            .find(|i| i.id == entry.id)
            .map(|i| i.version_id);
        super::admin_publishing::schedule_hooks(
            &state,
            ws,
            entry.id,
            EntryEvent::Published,
            version,
        )
        .await;
    }
    super::admin_publishing::schedule_search(&state, ws).await;
    Ok((
        StatusCode::CREATED,
        Json(ImportResult { entries: imported }),
    ))
}
