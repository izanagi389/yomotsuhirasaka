//! 管理API: 回答の語り口と、訪問者向けの「情報なし」の文（09-qa-chatbot.md「語り口」、Q24, Q25、M4）。
//!
//! | メソッド | パス | 内容 | 誰が |
//! |---|---|---|---|
//! | GET | /api/admin/workspaces/{ws}/answer-settings | 丁寧さ・長さ・自由記述（スイッチと文）・ロケールごとの「情報なし」の文 | 重い権限 manageLlm を持つ人・サーバー管理者 |
//! | PUT | /api/admin/workspaces/{ws}/answer-settings | 書いた項目だけを変える | 同上（再認証） |
//!
//! - 「情報なし」の文は、テンプレートのロケールだけを受け付ける（空・長すぎる・制御文字は拒否）。
//! - 自由記述は [`settings::MAX_FREE_PROMPT_CHARS`] 文字まで。使う設定にするなら空にできない。
//! - 答えない質問の決まり（`refusal`。拒否する語と組み込みの種類）も、ここで変える（09-qa-chatbot.md「拒否する語」）。
//! - 変更は監査ログに残す。残すのは変えた項目の名前と拒否する語の件数だけで、文や語の中身は残さない（原則7）。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use super::AppState;
use super::admin_entries::{db, parse, readable};
use super::admin_publishing::holder;
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::access::Permission;
use crate::answer::settings::{self, Change, View};
use crate::audit::action;
use crate::workspace_db;

pub async fn get(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<View>, ApiError> {
    let ws = parse(&ws)?;
    holder(&state, &current, ws, Permission::ManageLlm).await?;
    let applied = readable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    let out = settings::view(&mut tx, &applied.manifest).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}

pub async fn put(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    JsonBody(change): JsonBody<Change>,
) -> Result<Json<View>, ApiError> {
    let ws = parse(&ws)?;
    holder(&state, &current, ws, Permission::ManageLlm).await?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    let applied = readable(&state, ws).await?;
    let db = db(&state, ws).await?;
    let fields = change.fields();
    let mut tx = workspace_db::begin(&state.pool, &db).await?;
    if let Err(problems) = settings::apply(&mut tx, &applied.manifest, &change).await? {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_answer_settings",
            format!("The answer settings are not valid: {}", problems.join("; ")),
        ));
    }
    let out = settings::view(&mut tx, &applied.manifest).await?;
    if !fields.is_empty() {
        workspace_db::audit_full(
            &mut tx,
            &db,
            Some(ws),
            Some(current.user_id),
            action::ANSWER_SETTINGS_CHANGED,
            &[],
            false,
            // 拒否する語は、語そのものではなく件数だけを残す（語が秘密の名前でありうるため）
            Some(&serde_json::json!({ "fields": fields, "refusalWords": out.refusal.words.len() })),
        )
        .await?;
    }
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(out))
}
