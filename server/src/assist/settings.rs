//! 編集者向けのAI支援の設定（ワークスペースのスキーマの `assist_settings`。1行だけ）。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    /// 支援を使うか（既定は使わない）。
    pub enabled: bool,
}

pub async fn get(conn: &mut PgConnection) -> Result<Settings> {
    let enabled: Option<bool> = sqlx::query_scalar("SELECT enabled FROM assist_settings")
        .fetch_optional(&mut *conn)
        .await?;
    Ok(Settings {
        enabled: enabled.unwrap_or(false),
    })
}

pub async fn set(conn: &mut PgConnection, settings: Settings) -> Result<()> {
    sqlx::query(
        "INSERT INTO assist_settings (singleton, enabled) VALUES (true, $1)
         ON CONFLICT (singleton) DO UPDATE SET enabled = $1",
    )
    .bind(settings.enabled)
    .execute(&mut *conn)
    .await?;
    Ok(())
}
