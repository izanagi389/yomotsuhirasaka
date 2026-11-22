//! 記事: 本文の検証、フィールド、下書き（03-content-and-editing.md）。

pub mod body;
pub mod corrections;
pub mod entries;
pub mod erase;
pub mod fields;
pub mod freshness;
pub mod hooks;
pub mod html;
pub mod import;
pub mod locks;
pub mod previews;
pub mod publishing;
pub mod redirects;
pub mod rerender;
pub mod reviews;
pub mod scheduling;
pub mod sections;
pub mod settings;
pub mod translations;
pub mod urls;

use anyhow::{Context, Result};
use sqlx::PgPool;
use uuid::Uuid;

use crate::schema::deploy::load;
use crate::schema::manifest::Manifest;

/// ワークスペースに適用済みのスキーマ。
pub struct Applied {
    pub template: String,
    /// テンプレートをデプロイした日時（配信のスキーマのキャッシュのキー）。
    pub deployed_at: String,
    pub manifest: Manifest,
}

/// ワークスペースに適用済みのスキーマ（まだ何も適用していなければ None）。
/// 適用済みのワークスペースのスキーマは、いつもテンプレートの最新のマニフェストと同じ（デプロイは全部適用するか全部やめるか）。
pub async fn applied_schema(pool: &PgPool, workspace_id: Uuid) -> Result<Option<Applied>> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT template, schema_state FROM yomi.workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_optional(pool)
            .await?;
    let Some((template, state)) = row else {
        return Ok(None);
    };
    if state != "ok" {
        return Ok(None);
    }
    let deployed = load(pool, &template)
        .await?
        .context("the applied template is missing")?;
    Ok(Some(Applied {
        template,
        deployed_at: deployed.deployed_at,
        manifest: deployed.manifest,
    }))
}
