//! 管理API: ワークスペースの設定（yomi.workspace_settings。行がなければ既定の値）。
//!
//! | メソッド | パス | 内容 | 誰が |
//! |---|---|---|---|
//! | PATCH | /api/admin/workspaces/{ws}/settings | 設定の変更（`{takeOverPolicy?, previewOrigins?, previewPath?, crest?}`。送った項目だけ変える） | 管理者・サーバー管理者（多要素認証・再認証） |
//!
//! - 編集の引き継ぎを誰ができるか（03-content-and-editing.md「同時編集（Q73）」、計画の10番）: `anyone` か `permission`。
//! - プレビューを許すサイトのオリジン（03「プレビュー（Q72）」、13-security.md「ライブプレビュー」、計画の13番）:
//!   記事の画面の CSP の frame-src と、ライブプレビューのメッセージの送り元の照合に使う。
//!   https のオリジンだけ（http は localhost だけ）。パス・クエリ・利用者名などは受け付けない。最大10件。
//! - サイトのプレビュー用ルートのパス（既定 `/_yomi/preview`）: 管理画面は「オリジン ＋ パス ＋ `?token=…`」を開く。
//!   `/` で始まり、英数字と `-` `_` `.` `/` だけ（`//` と `..` は受け付けない）。最大100文字。
//! - ワークスペースの紋（`{motif, color}`。`null` で選ぶのをやめ、管理画面が ID から決める既定に戻す）: 管理画面で
//!   ワークスペースを見分けるための飾り。形は [`CREST_MOTIFS`]、色は [`CREST_COLORS`] から選ぶ。
//! - 変更は監査ログに残す。値は1件の取得（admin_workspaces.rs）で所属者に返す（紋は一覧でも返す）。

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

/// 紋の形（管理画面の admin/app/utils/kamon.ts と同じ並び。DB の検査制約とも同じ）。
pub const CREST_MOTIFS: &[&str] = &[
    "shippo", "asanoha", "hishi", "sakura", "ume", "kikyo", "fuji", "take", "nami", "kumo", "ogi",
    "kuyo",
];
/// 紋の色（藍・浅葱・朱・山吹・青磁・鼠）。
pub const CREST_COLORS: &[&str] = &["ai", "asagi", "shu", "yamabuki", "seiji", "nezumi"];

/// ワークスペースの紋。
#[derive(Deserialize, serde::Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Crest {
    pub motif: String,
    pub color: String,
}

/// 送られていない（None）と、null（Some(None)）を分けて受け取る。
fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<Crest>>, D::Error> {
    Option::<Crest>::deserialize(d).map(Some)
}

/// プレビューを許すサイトのオリジンの上限。
pub const MAX_PREVIEW_ORIGINS: usize = 10;

/// 引き継ぎの設定（行がなければ既定の anyone）。
pub(super) async fn take_over_policy_of(pool: &sqlx::PgPool, ws: Uuid) -> Result<String, ApiError> {
    let policy: Option<String> = sqlx::query_scalar(
        "SELECT take_over_policy FROM yomi.workspace_settings WHERE workspace_id = $1",
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
        "SELECT preview_origins FROM yomi.workspace_settings WHERE workspace_id = $1",
    )
    .bind(ws)
    .fetch_optional(pool)
    .await
    .map_err(anyhow::Error::from)?;
    Ok(origins.unwrap_or_default())
}

/// サイトのプレビュー用ルートのパス（行がなければ既定）。
pub(super) async fn preview_path_of(pool: &sqlx::PgPool, ws: Uuid) -> Result<String, ApiError> {
    let path: Option<String> = sqlx::query_scalar(
        "SELECT preview_path FROM yomi.workspace_settings WHERE workspace_id = $1",
    )
    .bind(ws)
    .fetch_optional(pool)
    .await
    .map_err(anyhow::Error::from)?;
    Ok(path.unwrap_or_else(|| DEFAULT_PREVIEW_PATH.to_owned()))
}

/// サイトのプレビュー用ルートの既定のパス（公式のNuxtモジュールの既定）。
pub const DEFAULT_PREVIEW_PATH: &str = "/_yomi/preview";

/// 受け付けるパスか。
pub fn valid_preview_path(path: &str) -> bool {
    path.len() <= 100
        && path.starts_with('/')
        && !path.contains("//")
        && !path.contains("..")
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
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
    /// サイトのプレビュー用ルートのパス。
    #[serde(default)]
    preview_path: Option<String>,
    /// ワークスペースの紋（`null` で既定に戻す）。
    #[serde(default, deserialize_with = "present")]
    crest: Option<Option<Crest>>,
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
    if let Some(p) = &req.preview_path
        && !valid_preview_path(p)
    {
        return Err(ApiError::bad_request(
            "`previewPath` must be a path such as /_yomi/preview (letters, digits, - _ . / only).",
        ));
    }
    if let Some(Some(c)) = &req.crest
        && (!CREST_MOTIFS.contains(&c.motif.as_str()) || !CREST_COLORS.contains(&c.color.as_str()))
    {
        return Err(ApiError::bad_request(format!(
            "`crest.motif` must be one of {} and `crest.color` one of {}.",
            CREST_MOTIFS.join(", "),
            CREST_COLORS.join(", ")
        )));
    }
    if req.take_over_policy.is_none()
        && origins.is_none()
        && req.preview_path.is_none()
        && req.crest.is_none()
    {
        return Err(ApiError::bad_request(
            "Send at least one setting to change.",
        ));
    }
    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    sqlx::query(
        "INSERT INTO yomi.workspace_settings
           (workspace_id, take_over_policy, preview_origins, preview_path, crest_motif, crest_color)
         VALUES ($1, COALESCE($2, 'anyone'), COALESCE($3, '{}'), COALESCE($4, '/_yomi/preview'), $6, $7)
         ON CONFLICT (workspace_id) DO UPDATE SET
           take_over_policy = COALESCE($2, yomi.workspace_settings.take_over_policy),
           preview_origins = COALESCE($3, yomi.workspace_settings.preview_origins),
           preview_path = COALESCE($4, yomi.workspace_settings.preview_path),
           crest_motif = CASE WHEN $5 THEN $6 ELSE yomi.workspace_settings.crest_motif END,
           crest_color = CASE WHEN $5 THEN $7 ELSE yomi.workspace_settings.crest_color END",
    )
    .bind(ws)
    .bind(&req.take_over_policy)
    .bind(&origins)
    .bind(&req.preview_path)
    .bind(req.crest.is_some())
    .bind(req.crest.as_ref().and_then(|c| c.as_ref().map(|c| c.motif.clone())))
    .bind(req.crest.as_ref().and_then(|c| c.as_ref().map(|c| c.color.clone())))
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
    fn preview_paths() {
        for ok in ["/_yomi/preview", "/preview", "/a-b_c.d/e"] {
            assert!(valid_preview_path(ok), "{ok}");
        }
        for bad in [
            "",
            "_yomi/preview",
            "//evil.example",
            "/a//b",
            "/a/../b",
            "/a?x=1",
            "/a#b",
            "/a b",
            "/%2e%2e",
        ] {
            assert!(!valid_preview_path(bad), "{bad}");
        }
        assert!(!valid_preview_path(&format!("/{}", "a".repeat(100))));
    }

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
