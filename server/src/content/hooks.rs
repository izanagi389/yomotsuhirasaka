//! フックの実行（04-hooks.md、Q12）。種類と受け取る値は `schema::manifest::Hooks` を参照。
//!
//! - 保存前のフック（beforeSave）は、フィールドの直しを返す。直しは送られた変更に重ね、Rustの検査にもう一度通して保存する。
//!   フックが失敗したとき、直しが検査に通らないときは、直さずに保存して知らせる（自動保存を止めて下書きを失わせないため）。
//! - 公開前のフック（beforePublish）は `publishing.rs` で動かす。
//! - 非同期のフック（afterSave・afterPublish・afterUnpublish）はジョブで動かし、失敗したらやり直す（原則5）。
//!   保存後のフックは、記事ごとにまとめ、保存が止まってから1回だけ動かす。
//!   フックのジョブは、ワークスペースごとに1つずつ実行する（Q120）。
//! - ジョブの中身（記事のIDと種類）は暗号化して保存する。フックの入力と出力はログに出さない（原則7）。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::{applied_schema, entries, fields, publishing};
use crate::egress::Egress;
use crate::hook_secrets;
use crate::jobs::Outcome;
use crate::keys::KeyRing;
use crate::schema::manifest::{ContentType, Manifest};
use crate::script::{Engine, Failure, Host, Permission, Script};
use crate::webhooks::EntryEvent;
use crate::workspace_db;

/// フックのジョブの種類。
pub const JOB_KIND: &str = "hook";
/// 非同期のフックを試す回数。
const MAX_ATTEMPTS: i32 = 5;
/// 保存後のフックを動かすまでに、保存が止まっているのを待つ時間（秒）。
pub const AFTER_SAVE_QUIET_SECONDS: f64 = 30.0;

/// 保存前のフックを動かし、保存する変更を返す。2つ目はフックが失敗したときの理由（そのときは直さない）。
///
/// - `entry`: フックに渡す記事のうち、フィールド以外（`{id, contentType, locale}`）
/// - `current`: 保存の前の下書きのフィールド（新しい記事なら空）
/// - `changes`: 送られた変更
pub async fn before_save(
    engine: &Engine,
    host: Host,
    manifest: &Manifest,
    ct: &ContentType,
    entry: Map<String, Value>,
    current: &Map<String, Value>,
    changes: Map<String, Value>,
) -> (Map<String, Value>, Option<Failure>) {
    let Some(hook) = &ct.hooks.before_save else {
        return (changes, None);
    };
    let mut after = current.clone();
    after.extend(changes.clone());
    let mut entry = entry;
    entry.insert("fields".to_owned(), Value::Object(after));
    let patch = match engine
        .before_save(hook, host, &json!({ "entry": entry }))
        .await
    {
        Ok(patch) if patch.is_empty() => return (changes, None),
        Ok(patch) => patch,
        Err(f) => return (changes, Some(f)),
    };
    let mut patched = changes.clone();
    patched.extend(patch);
    // 直しもRustの検査に通す。通らなければ、フックの返した値が正しくないものとして直さない
    match fields::convert_all(manifest, ct, &patched) {
        Ok(_) => (patched, None),
        Err(_) => (changes, Some(Failure::InvalidResult)),
    }
}

/// 非同期のフックの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum After {
    Save,
    Publish,
    Unpublish,
}

impl After {
    fn script(self, ct: &ContentType) -> Option<&Script> {
        match self {
            After::Save => ct.hooks.after_save.as_ref(),
            After::Publish => ct.hooks.after_publish.as_ref(),
            After::Unpublish => ct.hooks.after_unpublish.as_ref(),
        }
    }
}

/// フックに渡すイベント。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Event {
    /// 受け手が同じイベントを二度処理しないための目印。
    id: Uuid,
    #[serde(rename = "type")]
    kind: String,
    workspace_id: Uuid,
    entry_id: Uuid,
    locale: String,
    version_id: Option<Uuid>,
    url: Option<String>,
}

/// フックのジョブの中身。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Job {
    hook: After,
    content_type: String,
    event: Event,
}

/// 記事の種類・ロケール・公開URL（削除済みの記事も読む）。
async fn entry_row(
    pool: &PgPool,
    workspace_id: Uuid,
    entry_id: Uuid,
) -> Result<Option<(String, String, Option<String>)>> {
    let Some(db) = workspace_db::lookup(pool, workspace_id).await? else {
        return Ok(None);
    };
    let mut tx = workspace_db::begin(pool, &db).await?;
    let row = sqlx::query_as("SELECT content_type, locale, url FROM entries WHERE id = $1")
        .bind(entry_id)
        .fetch_optional(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(row)
}

/// 保存後のフックを積む（保存が止まってから記事ごとに1回）。フックがなければ何もしない。
pub async fn schedule_after_save(
    pool: &PgPool,
    keys: &KeyRing,
    ct: &ContentType,
    workspace_id: Uuid,
    entry_id: Uuid,
    locale: &str,
) -> Result<()> {
    if ct.hooks.after_save.is_none() {
        return Ok(());
    }
    let job = Job {
        hook: After::Save,
        content_type: ct.name.clone(),
        event: Event {
            id: Uuid::new_v4(),
            kind: "entry.saved".to_owned(),
            workspace_id,
            entry_id,
            locale: locale.to_owned(),
            version_id: None,
            url: None,
        },
    };
    crate::jobs::enqueue_debounced(
        pool,
        keys,
        Some(workspace_id),
        JOB_KIND,
        &serde_json::to_vec(&job)?,
        MAX_ATTEMPTS,
        &format!("hook:afterSave:{workspace_id}:{entry_id}"),
        AFTER_SAVE_QUIET_SECONDS,
    )
    .await
}

/// 公開後・非公開の後のフックを積む（操作を確定した後に呼ぶ）。フックがなければ何もしない。
pub async fn schedule_after(
    pool: &PgPool,
    keys: &KeyRing,
    workspace_id: Uuid,
    entry_id: Uuid,
    event: EntryEvent,
    version_id: Option<Uuid>,
) -> Result<()> {
    let hook = match event {
        EntryEvent::Published | EntryEvent::Updated | EntryEvent::Corrected => After::Publish,
        EntryEvent::Unpublished | EntryEvent::Deleted => After::Unpublish,
        _ => return Ok(()),
    };
    let Some((content_type, locale, url)) = entry_row(pool, workspace_id, entry_id).await? else {
        return Ok(());
    };
    let Some(applied) = applied_schema(pool, workspace_id).await? else {
        return Ok(());
    };
    if applied
        .manifest
        .content_type(&content_type)
        .and_then(|ct| hook.script(ct))
        .is_none()
    {
        return Ok(());
    }
    let job = Job {
        hook,
        content_type,
        event: Event {
            id: Uuid::new_v4(),
            kind: event.name().to_owned(),
            workspace_id,
            entry_id,
            locale,
            version_id,
            url,
        },
    };
    crate::jobs::enqueue(
        pool,
        keys,
        Some(workspace_id),
        JOB_KIND,
        &serde_json::to_vec(&job)?,
        MAX_ATTEMPTS,
    )
    .await?;
    Ok(())
}

/// フックのジョブを1つ実行する。テンプレートが変わってフックがなくなっていれば、何もせずに終える。
pub async fn run(
    pool: &PgPool,
    keys: &KeyRing,
    engine: &Engine,
    egress: Option<&Egress>,
    payload: &[u8],
) -> Result<Outcome> {
    let Ok(job) = serde_json::from_slice::<Job>(payload) else {
        return Ok(Outcome::Fail("invalid_payload"));
    };
    let ws = job.event.workspace_id;
    let (Some(applied), Some(db)) = (
        applied_schema(pool, ws).await?,
        workspace_db::lookup(pool, ws).await?,
    ) else {
        return Ok(Outcome::Done);
    };
    let Some(script) = applied
        .manifest
        .content_type(&job.content_type)
        .and_then(|ct| job.hook.script(ct))
    else {
        return Ok(Outcome::Done);
    };
    let entry = {
        let mut tx = workspace_db::begin(pool, &db).await?;
        let entry = match job.hook {
            After::Save => {
                match entries::draft(&mut tx, &applied.manifest, job.event.entry_id).await? {
                    Some(draft) => Some(publishing::hook_entry(&draft)),
                    // 削除された記事の保存後のフックは動かさない
                    None => return Ok(Outcome::Done),
                }
            }
            After::Publish => published_entry(&mut tx, job.event.entry_id).await?,
            After::Unpublish => None,
        };
        tx.commit().await?;
        entry
    };
    let mut host = Host::new(pool.clone(), db);
    if script.permissions.contains(&Permission::HttpFetch)
        && let Some(egress) = egress
    {
        host = host.with_fetch(egress.clone(), &script.allowed_hosts);
    }
    if script.permissions.contains(&Permission::SecretsRead) {
        host = host.with_secrets(hook_secrets::load(pool, keys, ws, &script.secrets).await?);
    }
    let input = json!({ "event": job.event, "entry": entry });
    Ok(match engine.after(script, host, &input).await {
        Ok(()) => Outcome::Done,
        Err(f) => Outcome::Retry(f.code()),
    })
}

/// 公開中の版の記事（`{id, contentType, locale, fields}`）。公開されていなければ None。
async fn published_entry(conn: &mut sqlx::PgConnection, entry_id: Uuid) -> Result<Option<Value>> {
    let row: Option<(Uuid, String, String, Option<String>)> = sqlx::query_as(
        "SELECT e.id, e.content_type, e.locale, v.fields::text
         FROM entries e JOIN versions v ON v.id = e.published_version_id
         WHERE e.id = $1 AND e.deleted_at IS NULL",
    )
    .bind(entry_id)
    .fetch_optional(conn)
    .await?;
    let Some((id, content_type, locale, Some(fields))) = row else {
        return Ok(None);
    };
    let fields: Value = serde_json::from_str(&fields)?;
    Ok(Some(
        json!({ "id": id, "contentType": content_type, "locale": locale, "fields": fields }),
    ))
}
