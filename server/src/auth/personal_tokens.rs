//! CLI用の個人トークン（Q91, Q110, Q119）。
//!
//! - 形は `cms_pat_<ランダムな43文字>`。発行時に一度だけ渡し、DBにはハッシュだけを保存する（INV-KEY-01）。
//! - 有効期限は必須（最長365日）。範囲（read・write・deploy）と、使えるワークスペースを発行時に決める。
//! - 有効な権限は、トークンの範囲と、リクエストの時点での本人のロールの重なりだけ。
//!   本人がワークスペースから外されたり、無効にされたりしたら、直ちに使えなくなる（照合のたびに確かめる）。
//! - `Authorization: Bearer` でだけ受け付ける。URLに載ったものは拒否する（INV-AUTH-11）。

use anyhow::Result;
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use super::session::{Current, TokenGrant};
use super::token;

/// 個人トークンの接頭辞。URLに載ったかどうかの検査にも使う。
pub const PREFIX: &str = "cms_pat_";
/// 配信APIのキーの接頭辞（キーは次の段階で追加する。URLの検査はここで先に行う）。
pub const API_KEY_PREFIX: &str = "cms_key_";
/// 有効期限の上限（日）。
pub const MAX_LIFETIME_DAYS: i32 = 365;
/// 使った時刻の記録の間隔（秒）。毎回は書き込まない。
const TOUCH_INTERVAL_SECONDS: f64 = 60.0;

/// トークンの範囲。
pub const SCOPES: [&str; 3] = ["read", "write", "deploy"];

/// 新しく発行したトークン。`secret` はこの場でだけ手に入る。
pub struct Issued {
    pub id: Uuid,
    pub secret: String,
    pub expires_at: String,
}

const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;

/// 有効期限の誤り。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpiryError {
    /// 時刻として読めない。
    Invalid,
    /// 過去、または最長の期間（365日）より先。
    OutOfRange,
}

/// 有効期限を確かめる（時刻の解釈と比較はPostgreSQLで行う）。`expires_at` は RFC 3339 の時刻。
pub async fn check_expiry<'e>(
    executor: impl PgExecutor<'e>,
    expires_at: &str,
) -> Result<Result<(), ExpiryError>> {
    if !crate::timefmt::is_rfc3339(expires_at) {
        return Ok(Err(ExpiryError::Invalid));
    }
    let checked = sqlx::query_scalar::<_, bool>(
        "SELECT $1::timestamptz > now() AND $1::timestamptz <= now() + make_interval(days => $2)",
    )
    .bind(expires_at)
    .bind(MAX_LIFETIME_DAYS)
    .fetch_one(executor)
    .await;
    match checked {
        Ok(true) => Ok(Ok(())),
        Ok(false) => Ok(Err(ExpiryError::OutOfRange)),
        // 22007: invalid_datetime_format、22008: datetime_field_overflow
        Err(e)
            if e.as_database_error()
                .and_then(|d| d.code())
                .is_some_and(|c| c == "22007" || c == "22008") =>
        {
            Ok(Err(ExpiryError::Invalid))
        }
        Err(e) => Err(e.into()),
    }
}

/// トークンを発行する。有効期限は [`check_expiry`] で確かめておくこと。
pub async fn issue<'e>(
    executor: impl PgExecutor<'e>,
    user_id: Uuid,
    name: &str,
    scopes: &[String],
    workspaces: &[Uuid],
    expires_at: &str,
) -> Result<Issued> {
    let secret = format!("{PREFIX}{}", token::generate());
    let id = Uuid::new_v4();
    let display_prefix: String = secret.chars().take(PREFIX.len() + 4).collect();
    let expires_at: String = sqlx::query_scalar(crate::sql!(
        "INSERT INTO cms.personal_tokens (id, user_id, name, token_hash, display_prefix, scopes, workspace_ids, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8::timestamptz)
         RETURNING to_char(expires_at AT TIME ZONE 'UTC', {RFC3339})"
    ))
    .bind(id)
    .bind(user_id)
    .bind(name)
    .bind(token::digest(&secret))
    .bind(&display_prefix)
    .bind(scopes)
    .bind(workspaces)
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
    pub workspace_ids: Vec<Uuid>,
    pub created_at: String,
    pub expires_at: String,
    pub last_used_at: Option<String>,
}

type SummaryRow = (
    Uuid,
    String,
    String,
    Vec<String>,
    Vec<Uuid>,
    String,
    String,
    Option<String>,
);

/// 本人の有効なトークンの一覧。
pub async fn list(pool: &PgPool, user_id: Uuid) -> Result<Vec<Summary>> {
    let rows: Vec<SummaryRow> = sqlx::query_as(crate::sql!(
        "SELECT id, name, display_prefix, scopes, workspace_ids,
                to_char(created_at AT TIME ZONE 'UTC', {RFC3339}),
                to_char(expires_at AT TIME ZONE 'UTC', {RFC3339}),
                to_char(last_used_at AT TIME ZONE 'UTC', {RFC3339})
         FROM cms.personal_tokens
         WHERE user_id = $1 AND revoked_at IS NULL AND expires_at > now()
         ORDER BY created_at DESC"
    ))
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                name,
                display_prefix,
                scopes,
                workspace_ids,
                created_at,
                expires_at,
                last_used_at,
            )| Summary {
                id,
                name,
                display_prefix,
                scopes,
                workspace_ids,
                created_at,
                expires_at,
                last_used_at,
            },
        )
        .collect())
}

/// 本人のトークンを失効させる。本人のものでなければ false。
pub async fn revoke(pool: &PgPool, user_id: Uuid, id: Uuid) -> Result<bool> {
    let done = sqlx::query(
        "UPDATE cms.personal_tokens SET revoked_at = now()
         WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

type ResolvedRow = (
    Uuid,
    Vec<String>,
    Vec<Uuid>,
    bool,
    Uuid,
    String,
    bool,
    bool,
    Option<String>,
);

/// `Authorization: Bearer` で受け取ったトークンを照合する。無効なら None。
pub async fn resolve(pool: &PgPool, presented: &str) -> Result<Option<Current>> {
    if !presented.starts_with(PREFIX) {
        return Ok(None);
    }
    let row: Option<ResolvedRow> = sqlx::query_as(
        "SELECT t.id, t.scopes, t.workspace_ids,
                t.last_used_at IS NULL OR t.last_used_at < now() - make_interval(secs => $2),
                u.id, u.email, u.is_server_admin, u.totp_enabled_at IS NOT NULL, u.locale
         FROM cms.personal_tokens t JOIN cms.users u ON u.id = t.user_id
         WHERE t.token_hash = $1 AND t.revoked_at IS NULL AND t.expires_at > now() AND NOT u.disabled",
    )
    .bind(token::digest(presented))
    .bind(TOUCH_INTERVAL_SECONDS)
    .fetch_optional(pool)
    .await?;
    let Some((
        token_id,
        scopes,
        workspaces,
        stale,
        user_id,
        email,
        is_server_admin,
        mfa_enabled,
        locale,
    )) = row
    else {
        return Ok(None);
    };
    if stale {
        sqlx::query("UPDATE cms.personal_tokens SET last_used_at = now() WHERE id = $1")
            .bind(token_id)
            .execute(pool)
            .await?;
    }
    Ok(Some(Current::from_token(
        user_id,
        email,
        is_server_admin,
        mfa_enabled,
        locale.and_then(|l| l.parse().ok()),
        TokenGrant {
            token_id,
            scopes,
            workspaces,
        },
    )))
}

/// URLのクエリに、キーやトークンらしい値が含まれているか（INV-AUTH-11）。
/// パーセントエンコードされていても見つけられるよう、`%5F`（`_`）を戻してから探す。
pub fn credential_in_query(query: &str) -> bool {
    let lower = query.to_ascii_lowercase().replace("%5f", "_");
    lower.contains(PREFIX) || lower.contains(API_KEY_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_in_urls_are_detected() {
        assert!(credential_in_query("token=cms_pat_abc"));
        assert!(credential_in_query("x=1&access_token=CMS_PAT_abc"));
        assert!(credential_in_query("key=cms%5Fkey%5Fabc"));
        assert!(credential_in_query("api_key=cms_key_abc"));
        assert!(!credential_in_query("q=hello&page=2"));
        assert!(!credential_in_query(""));
    }
}
