//! ホスト関数（04-hooks.md「外部へのアクセス」、Q11）。
//!
//! - 関数が権限として宣言したものだけを、グローバルの `cms` に置く（INV-HOOK-02）。
//! - DBは、関数を動かしているワークスペースのロールに切り替えたトランザクションでだけ読む。他のワークスペースのスキーマには、
//!   DBの権限でも触れられない（INV-HOOK-03、INV-DB-01）。
//! - 待つ時間も、関数の実行時間の上限に含める。
//!
//! | 権限 | 関数 | 内容 |
//! |---|---|---|
//! | `content.read` | `cms.content.get(id)` | 同じワークスペースの公開中の記事（`{id, contentType, locale, fields}`）。なければ null |

use std::time::Instant;

use rquickjs::{Ctx, Exception, Function, Object, Value as JsValue};
use serde_json::json;
use sqlx::PgPool;
use tokio::runtime::Handle;
use uuid::Uuid;

use super::Permission;
use crate::workspace_db::{self, WorkspaceDb};

/// 関数を動かすワークスペース。ホスト関数は、このワークスペースのデータだけを読む。
#[derive(Clone)]
pub struct Host {
    pool: PgPool,
    db: WorkspaceDb,
}

impl Host {
    pub fn new(pool: PgPool, db: WorkspaceDb) -> Self {
        Self { pool, db }
    }

    pub(super) fn bind(self, handle: Handle) -> Bound {
        Bound { host: self, handle }
    }
}

/// 実行するスレッドから、tokio のランタイムでDBを読むための組。
pub(super) struct Bound {
    host: Host,
    handle: Handle,
}

/// 宣言された権限のホスト関数を、グローバルの `cms` に置く。権限がなければ `cms` も置かない。
pub(super) fn install<'js>(
    ctx: &Ctx<'js>,
    permissions: &[Permission],
    bound: Bound,
    deadline: Instant,
) -> rquickjs::Result<()> {
    if permissions.is_empty() {
        return Ok(());
    }
    let cms = Object::new(ctx.clone())?;
    if permissions.contains(&Permission::ContentRead) {
        let content = Object::new(ctx.clone())?;
        let Bound { host, handle } = bound;
        let get = Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, id: String| -> rquickjs::Result<JsValue<'js>> {
                let Ok(id) = Uuid::parse_str(&id) else {
                    return Ok(JsValue::new_null(ctx));
                };
                let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                    return Err(Exception::throw_internal(&ctx, "time limit"));
                };
                let found = handle.block_on(tokio::time::timeout(left, read_entry(&host, id)));
                match found {
                    Ok(Ok(Some(json))) => ctx.json_parse(json),
                    Ok(Ok(None)) => Ok(JsValue::new_null(ctx)),
                    Ok(Err(e)) => {
                        tracing::warn!(workspace_id = %host.db.workspace_id, "cms.content.get failed: {e:#}");
                        Err(Exception::throw_internal(&ctx, "cannot read the entry"))
                    }
                    Err(_) => Err(Exception::throw_internal(&ctx, "time limit")),
                }
            },
        )?;
        content.set("get", get)?;
        cms.set("content", content)?;
    }
    ctx.globals().set("cms", cms)
}

/// 公開中の記事を、公開した版のフィールドで返す（下書きの内容は公開URLに使わない）。
async fn read_entry(host: &Host, id: Uuid) -> anyhow::Result<Option<String>> {
    let mut tx = workspace_db::begin(&host.pool, &host.db).await?;
    let row: Option<(Uuid, String, String, Option<String>)> = sqlx::query_as(
        "SELECT e.id, e.content_type, e.locale, v.fields::text
         FROM entries e JOIN versions v ON v.id = e.published_version_id
         WHERE e.id = $1 AND e.deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    let Some((id, content_type, locale, Some(fields))) = row else {
        return Ok(None);
    };
    let fields: serde_json::Value = serde_json::from_str(&fields)?;
    Ok(Some(
        json!({ "id": id, "contentType": content_type, "locale": locale, "fields": fields })
            .to_string(),
    ))
}
