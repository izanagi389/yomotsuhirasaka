//! 記事: 本文の検証、フィールド、下書き（03-content-and-editing.md）。

pub mod body;
pub mod entries;
pub mod erase;
pub mod fields;
pub mod hooks;
pub mod import;
pub mod publishing;
pub mod sections;
pub mod settings;
pub mod translations;

use anyhow::{Context, Result};
use sqlx::PgPool;
use uuid::Uuid;

use crate::schema::deploy::load_version;
use crate::schema::manifest::Manifest;

/// ワークスペースに適用済みのスキーマ。
pub struct Applied {
    pub template: String,
    pub version: i32,
    /// 移行待ちか（Q53）。移行待ちの間は、編集の操作を受け付けない。
    pub pending: bool,
    pub manifest: Manifest,
}

/// ワークスペースに適用済みのスキーマ（まだ何も適用していなければ None）。
pub async fn applied_schema(pool: &PgPool, workspace_id: Uuid) -> Result<Option<Applied>> {
    let row: Option<(String, Option<i32>, String)> = sqlx::query_as(
        "SELECT template, schema_version, schema_state FROM cms.workspaces WHERE id = $1",
    )
    .bind(workspace_id)
    .fetch_optional(pool)
    .await?;
    let Some((template, Some(version), state)) = row else {
        return Ok(None);
    };
    let manifest = load_version(pool, &template, version)
        .await?
        .context("applied template version is missing")?;
    Ok(Some(Applied {
        template,
        version,
        pending: state == "pending",
        manifest,
    }))
}
