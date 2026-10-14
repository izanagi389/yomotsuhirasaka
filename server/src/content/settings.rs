//! 記事の公開設定のスイッチ（03-content-and-editing.md「管理画面の体験」、Q184）。
//!
//! M1では保存と、変更のWebhook（`entry.settings_changed`）だけ。効果はM2以降で足す。
//! 版には含めず、公開し直さずに切り替える。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

/// 公開設定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishSettings {
    /// チャットボットの回答の根拠に使う（既定: 使う）。
    pub use_for_chatbot: bool,
    /// 外部AI向けの素材に含める（既定: 含める）。
    pub include_in_ai_materials: bool,
    /// 学習への利用を拒否する（既定: 拒否しない）。
    pub opt_out_of_ai_training: bool,
}

/// 変更の要求（送られた項目だけを変える）。
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Change {
    pub use_for_chatbot: Option<bool>,
    pub include_in_ai_materials: Option<bool>,
    pub opt_out_of_ai_training: Option<bool>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        self.use_for_chatbot.is_none()
            && self.include_in_ai_materials.is_none()
            && self.opt_out_of_ai_training.is_none()
    }
}

type Row = (bool, bool, bool);

fn from_row(
    (use_for_chatbot, include_in_ai_materials, opt_out_of_ai_training): Row,
) -> PublishSettings {
    PublishSettings {
        use_for_chatbot,
        include_in_ai_materials,
        opt_out_of_ai_training,
    }
}

/// 公開設定を読む（削除済みの記事は None）。
pub async fn get(conn: &mut PgConnection, entry_id: Uuid) -> Result<Option<PublishSettings>> {
    let row: Option<Row> = sqlx::query_as(
        "SELECT use_for_chatbot, include_in_ai_materials, opt_out_of_ai_training
         FROM entries WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(entry_id)
    .fetch_optional(conn)
    .await?;
    Ok(row.map(from_row))
}

/// 公開設定を変える。記事がなければ None。変わったかどうかも返す。
pub async fn change(
    conn: &mut PgConnection,
    entry_id: Uuid,
    change: Change,
) -> Result<Option<(PublishSettings, bool)>> {
    let before: Option<Row> = sqlx::query_as(
        "SELECT use_for_chatbot, include_in_ai_materials, opt_out_of_ai_training
         FROM entries WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(entry_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(before) = before.map(from_row) else {
        return Ok(None);
    };
    let after = PublishSettings {
        use_for_chatbot: change.use_for_chatbot.unwrap_or(before.use_for_chatbot),
        include_in_ai_materials: change
            .include_in_ai_materials
            .unwrap_or(before.include_in_ai_materials),
        opt_out_of_ai_training: change
            .opt_out_of_ai_training
            .unwrap_or(before.opt_out_of_ai_training),
    };
    if after != before {
        sqlx::query(
            "UPDATE entries SET use_for_chatbot = $2, include_in_ai_materials = $3, opt_out_of_ai_training = $4
             WHERE id = $1",
        )
        .bind(entry_id)
        .bind(after.use_for_chatbot)
        .bind(after.include_in_ai_materials)
        .bind(after.opt_out_of_ai_training)
        .execute(&mut *conn)
        .await?;
    }
    Ok(Some((after, after != before)))
}
