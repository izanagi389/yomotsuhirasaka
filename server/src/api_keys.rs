//! 配信APIのキー（Q13, Q93, Q110, Q125, Q153）。
//!
//! - 形は `cms_key_<ランダムな43文字>`。発行時に一度だけ渡し、DBにはハッシュだけを保存する（INV-KEY-01）。
//! - ワークスペースごと。配信APIのURLのワークスペースIDと、キーのワークスペースを必ず照合する（Q125）。
//! - スコープ: `read`（公開版の読み取り）、`register`（クエリの登録）。用途（build・server）は回数制限の既定値だけに影響する。
//! - 有効期限（任意）を過ぎたキーと、失効したキーは使えない（INV-KEY-04）。

use anyhow::Result;
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use crate::auth::personal_tokens::API_KEY_PREFIX as PREFIX;
use crate::auth::token;

pub const SCOPES: [&str; 2] = ["read", "register"];
pub const PURPOSES: [&str; 2] = ["build", "server"];

const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;
/// 使った時刻の記録の間隔（秒）。毎回は書き込まない。
const TOUCH_INTERVAL_SECONDS: f64 = 60.0;

pub struct Issued {
    pub id: Uuid,
    pub secret: String,
    pub expires_at: Option<String>,
}

/// キーを発行する。`expires_at` は確かめ済みの RFC 3339 の時刻か None。
#[allow(clippy::too_many_arguments)]
pub async fn issue<'e>(
    executor: impl PgExecutor<'e>,
    workspace_id: Uuid,
    name: &str,
    scopes: &[String],
    purpose: &str,
    created_by: Uuid,
    expires_at: Option<&str>,
) -> Result<Issued> {
    let secret = format!("{PREFIX}{}", token::generate());
    let id = Uuid::new_v4();
    let display_prefix: String = secret.chars().take(PREFIX.len() + 4).collect();
    let expires_at: Option<String> = sqlx::query_scalar(crate::sql!(
        "INSERT INTO cms.api_keys (id, workspace_id, name, key_hash, display_prefix, scopes, purpose, created_by, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::timestamptz)
         RETURNING to_char(expires_at AT TIME ZONE 'UTC', {RFC3339})"
    ))
    .bind(id)
    .bind(workspace_id)
    .bind(name)
    .bind(token::digest(&secret))
    .bind(&display_prefix)
    .bind(scopes)
    .bind(purpose)
    .bind(created_by)
    .bind(expires_at)
    .fetch_one(executor)
    .await?;
    Ok(Issued {
        id,
        secret,
        expires_at,
    })
}

/// 一覧に出す情報（秘密の値は含めない）。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub id: Uuid,
    pub name: String,
    pub display_prefix: String,
    pub scopes: Vec<String>,
    pub purpose: String,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub last_used_at: Option<String>,
}

type SummaryRow = (
    Uuid,
    String,
    String,
    Vec<String>,
    String,
    String,
    Option<String>,
    Option<String>,
);

/// ワークスペースの有効なキーの一覧。
pub async fn list(pool: &PgPool, workspace_id: Uuid) -> Result<Vec<Summary>> {
    let rows: Vec<SummaryRow> = sqlx::query_as(crate::sql!(
        "SELECT id, name, display_prefix, scopes, purpose,
                to_char(created_at AT TIME ZONE 'UTC', {RFC3339}),
                to_char(expires_at AT TIME ZONE 'UTC', {RFC3339}),
                to_char(last_used_at AT TIME ZONE 'UTC', {RFC3339})
         FROM cms.api_keys
         WHERE workspace_id = $1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > now())
         ORDER BY created_at DESC"
    ))
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, name, display_prefix, scopes, purpose, created_at, expires_at, last_used_at)| {
                Summary {
                    id,
                    name,
                    display_prefix,
                    scopes,
                    purpose,
                    created_at,
                    expires_at,
                    last_used_at,
                }
            },
        )
        .collect())
}

/// キーを失効させる。そのワークスペースの有効なキーでなければ false。
pub async fn revoke(pool: &PgPool, workspace_id: Uuid, id: Uuid) -> Result<bool> {
    let done = sqlx::query(
        "UPDATE cms.api_keys SET revoked_at = now()
         WHERE id = $1 AND workspace_id = $2 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(workspace_id)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// 照合できたキー。
#[derive(Debug, Clone)]
pub struct Grant {
    pub key_id: Uuid,
    pub workspace_id: Uuid,
    pub scopes: Vec<String>,
    pub purpose: String,
}

impl Grant {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

/// キーを使えない理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    /// キーがない・形が違う・存在しない・期限切れ・失効。
    Invalid,
    /// URLのワークスペースとキーのワークスペースが違う（Q125）。
    WrongWorkspace,
    /// 必要なスコープがない。
    MissingScope,
}

type ResolvedRow = (Uuid, Uuid, Vec<String>, String, bool);

/// 配信APIのリクエストのキーを照合する。`presented` は `Authorization: Bearer` の値。
/// `url_workspace` はURLに含まれるワークスペースID、`scope` はその操作に要るスコープ。
pub async fn authenticate(
    pool: &PgPool,
    presented: Option<&str>,
    url_workspace: Uuid,
    scope: &str,
) -> Result<Result<Grant, Denied>> {
    let Some(presented) = presented.filter(|p| p.starts_with(PREFIX)) else {
        return Ok(Err(Denied::Invalid));
    };
    let row: Option<ResolvedRow> = sqlx::query_as(
        "SELECT id, workspace_id, scopes, purpose,
                last_used_at IS NULL OR last_used_at < now() - make_interval(secs => $2)
         FROM cms.api_keys
         WHERE key_hash = $1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > now())",
    )
    .bind(token::digest(presented))
    .bind(TOUCH_INTERVAL_SECONDS)
    .fetch_optional(pool)
    .await?;
    let Some((key_id, workspace_id, scopes, purpose, stale)) = row else {
        return Ok(Err(Denied::Invalid));
    };
    if workspace_id != url_workspace {
        return Ok(Err(Denied::WrongWorkspace));
    }
    let grant = Grant {
        key_id,
        workspace_id,
        scopes,
        purpose,
    };
    if !grant.has_scope(scope) {
        return Ok(Err(Denied::MissingScope));
    }
    if stale {
        sqlx::query("UPDATE cms.api_keys SET last_used_at = now() WHERE id = $1")
            .bind(key_id)
            .execute(pool)
            .await?;
    }
    Ok(Ok(grant))
}
