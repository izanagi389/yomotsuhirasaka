//! 管理API: ワークスペースの設定（cms.workspace_settings。行がなければ既定の値）。
//!
//! | メソッド | パス | 内容 | 誰が |
//! |---|---|---|---|
//! | PATCH | /api/admin/workspaces/{ws}/settings | 設定の変更（`{takeOverPolicy?, previewOrigins?}`。送った項目だけ変える） | 管理者・サーバー管理者（多要素認証・再認証） |
//!
//! - 編集の引き継ぎを誰ができるか（03-content-and-editing.md「同時編集（Q73）」、計画の10番）: `anyone` か `permission`。
//! - プレビューを許すサイトのオリジン（03「プレビュー（Q72）」、13-security.md「ライブプレビュー」、計画の13番）:
//!   記事の画面の CSP の frame-src と、ライブプレビューのメッセージの送り元の照合に使う。
//!   https のオリジンだけ（http は localhost だけ）。パス・クエリ・利用者名などは受け付けない。最大10件。
//! - 変更は監査ログに残す。値は1件の取得（admin_workspaces.rs）で所属者に返す。

use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use uuid::Uuid;

use super::AppState;
use super::admin_entries::parse;
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::access;
use crate::audit::{self, action};

/// プレビューを許すサイトのオリジンの上限。
pub const MAX_PREVIEW_ORIGINS: usize = 10;

/// 引き継ぎの設定（行がなければ既定の anyone）。
pub(super) async fn take_over_policy_of(pool: &sqlx::PgPool, ws: Uuid) -> Result<String, ApiError> {
    let policy: Option<String> = sqlx::query_scalar(
        "SELECT take_over_policy FROM cms.workspace_settings WHERE workspace_id = $1",
    )
    .bind(ws)
    .fetch_optional(pool)
    .await
    .map_err(anyhow::Error::from)?;
    Ok(policy.unwrap_or_else(|| "anyone".to_owned()))
}

/// プレビューを許すサイトのオリジン（行がなければ空）。
pub(super) async fn preview_origins_of(
    pool: &sqlx::PgPool,
    ws: Uuid,
) -> Result<Vec<String>, ApiError> {
    let origins: Option<Vec<String>> = sqlx::query_scalar(
        "SELECT preview_origins FROM cms.workspace_settings WHERE workspace_id = $1",
    )
    .bind(ws)
    .fetch_optional(pool)
    .await
    .map_err(anyhow::Error::from)?;
    Ok(origins.unwrap_or_default())
}

/// 受け付けるオリジンなら、正規化したもの（`https://host[:port]`）。
/// https か、http なら localhost（`localhost`・`*.localhost`・`127.0.0.1`・`[::1]`）だけ。パス（`/` 以外）・クエリ・
/// フラグメント・利用者名とパスワードがあれば受け付けない。
pub fn preview_origin(input: &str) -> Option<String> {
    let u = url::Url::parse(input.trim()).ok()?;
    let host = u.host_str()?;
    // ワイルドカードなどの記号は受け付けない（英数字・ハイフン・ドット、IPv6 の角かっことコロンだけ）
    if !host
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '[' | ']' | ':'))
    {
        return None;
    }
    let local = host == "localhost"
        || host.ends_with(".localhost")
        || host == "127.0.0.1"
        || host == "[::1]";
    let scheme_ok = u.scheme() == "https" || (u.scheme() == "http" && local);
    let bare = u.path() == "/"
        && u.query().is_none()
        && u.fragment().is_none()
        && u.username().is_empty()
        && u.password().is_none();
    (scheme_ok && bare).then(|| u.origin().ascii_serialization())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    /// 編集の引き継ぎ: `anyone`（下書きを編集できる人なら誰でも）か `permission`（重い権限を持つ人だけ）。
    #[serde(default)]
    take_over_policy: Option<String>,
    /// プレビューを許すサイトのオリジン（一覧全体を置き換える）。
    #[serde(default)]
    preview_origins: Option<Vec<String>>,
}

/// ワークスペースの設定の変更（管理者・多要素認証・再認証。監査ログに記録）。
pub async fn patch_settings(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    JsonBody(req): JsonBody<Settings>,
) -> Result<StatusCode, ApiError> {
    let ws = parse(&ws)?;
    access::workspace_admin(&state.pool, &current, ws).await??;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    if let Some(p) = &req.take_over_policy
        && !matches!(p.as_str(), "anyone" | "permission")
    {
        return Err(ApiError::bad_request(
            "`takeOverPolicy` must be \"anyone\" or \"permission\".",
        ));
    }
    let origins = match &req.preview_origins {
        None => None,
        Some(list) => {
            if list.len() > MAX_PREVIEW_ORIGINS {
                return Err(ApiError::bad_request(format!(
                    "`previewOrigins` can have at most {MAX_PREVIEW_ORIGINS} origins."
                )));
            }
            let mut out: Vec<String> = Vec::new();
            for o in list {
                let Some(normalized) = preview_origin(o) else {
                    return Err(ApiError::bad_request(
                        "Each of `previewOrigins` must be an https origin such as https://example.com (http only for localhost), without a path.",
                    ));
                };
                if !out.contains(&normalized) {
                    out.push(normalized);
                }
            }
            Some(out)
        }
    };
    if req.take_over_policy.is_none() && origins.is_none() {
        return Err(ApiError::bad_request(
            "Send at least one setting to change.",
        ));
    }
    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    sqlx::query(
        "INSERT INTO cms.workspace_settings (workspace_id, take_over_policy, preview_origins)
         VALUES ($1, COALESCE($2, 'anyone'), COALESCE($3, '{}'))
         ON CONFLICT (workspace_id) DO UPDATE SET
           take_over_policy = COALESCE($2, cms.workspace_settings.take_over_policy),
           preview_origins = COALESCE($3, cms.workspace_settings.preview_origins)",
    )
    .bind(ws)
    .bind(&req.take_over_policy)
    .bind(&origins)
    .execute(&mut *tx)
    .await
    .map_err(anyhow::Error::from)?;
    audit::record(
        &mut *tx,
        Some(ws),
        Some(current.user_id),
        action::WORKSPACE_SETTINGS_CHANGED,
        &[ws],
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_origins_are_normalized_and_restricted() {
        assert_eq!(
            preview_origin("https://Example.com/").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            preview_origin("https://example.com:443").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            preview_origin("https://example.com:8443").as_deref(),
            Some("https://example.com:8443")
        );
        assert_eq!(
            preview_origin("http://localhost:3000").as_deref(),
            Some("http://localhost:3000")
        );
        assert_eq!(
            preview_origin("http://site.localhost").as_deref(),
            Some("http://site.localhost")
        );
        for bad in [
            "http://example.com",
            "https://example.com/blog",
            "https://example.com/?a=1",
            "https://example.com/#x",
            "https://user:pw@example.com",
            "ftp://example.com",
            "javascript:alert(1)",
            "*.example.com",
            "https://*.example.com",
            "",
        ] {
            assert_eq!(preview_origin(bad), None, "{bad}");
        }
    }
}
