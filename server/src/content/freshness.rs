//! 鮮度: 最終確認日と確認期限（06-corrections-and-freshness.md「鮮度」、Q44, Q46）。
//! ワークスペースのロールに切り替えたトランザクションの中で呼ぶ。
//!
//! - 確認期限の日数の既定値は、コンテンツタイプの `review.intervalDays`（なければ期限なし）。記事ごとに日数を上書きでき、
//!   「期限なし」にもできる。
//! - 最終確認日の初めの値は最初に公開した日。公開し直しても進めず、「確認済み」の操作でだけ進める（版は作らない）。
//! - 期限切れかどうかは、読むたびに現在時刻で判定する（保存しない）。
//! - 確認者とメモは管理画面の中だけで使い、配信には最終確認日・確認期限・期限切れかだけを出す（INV-FRS-01）。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::schema::manifest::Manifest;

const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;
/// メモの長さの上限（文字）。
pub const MAX_MEMO_CHARS: usize = 2000;
/// 確認期限の日数の上限。
pub const MAX_INTERVAL_DAYS: i32 = 3650;

/// 配信にも出す鮮度。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Freshness {
    /// 最終確認日（公開したことがなければ None）。
    pub last_verified_at: Option<String>,
    /// 確認期限（期限なしなら None）。
    pub review_due_at: Option<String>,
    pub review_expired: bool,
}

/// 管理画面に出す鮮度（確認期限の上書きの状態を含む）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FreshnessDetail {
    #[serde(flatten)]
    pub freshness: Freshness,
    /// 記事ごとの上書き（日数）。None ならスキーマの既定。
    pub review_interval_days: Option<i32>,
    /// 記事ごとに「期限なし」にしたか。
    pub review_disabled: bool,
    /// スキーマの既定の日数。
    pub default_interval_days: Option<u32>,
    pub history: Vec<Verification>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Verification {
    pub verified_at: String,
    pub verified_by: Option<Uuid>,
    pub memo: String,
}

fn default_interval(manifest: &Manifest, content_type: &str) -> Option<u32> {
    manifest
        .content_type(content_type)?
        .review
        .as_ref()
        .map(|r| r.interval_days)
}

/// 記事の鮮度と上書きの状態を、現在時刻で求める。削除済みなら None。
async fn compute(
    conn: &mut PgConnection,
    default_interval: impl FnOnce(&str) -> Option<u32>,
    entry_id: Uuid,
) -> Result<Option<(Freshness, Option<i32>, bool, Option<u32>)>> {
    let row: Option<(String, Option<i32>, bool)> = sqlx::query_as(
        "SELECT content_type, review_interval_days, review_disabled FROM entries WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(entry_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((content_type, override_days, disabled)) = row else {
        return Ok(None);
    };
    let default = default_interval(&content_type);
    let days: Option<i32> = if disabled {
        None
    } else {
        override_days.or(default.map(|d| d as i32))
    };
    let (last, due, expired): (Option<String>, Option<String>, Option<bool>) = sqlx::query_as(crate::sql!(
        "WITH base AS (
             SELECT COALESCE(e.last_verified_at, (SELECT min(published_at) FROM versions WHERE entry_id = e.id)) AS last
             FROM entries e WHERE e.id = $1
         )
         SELECT to_char(last AT TIME ZONE 'UTC', {RFC3339}),
                to_char((last + make_interval(days => $2)) AT TIME ZONE 'UTC', {RFC3339}),
                last + make_interval(days => $2) < now()
         FROM base"
    ))
    .bind(entry_id)
    .bind(days)
    .fetch_one(&mut *conn)
    .await?;
    let freshness = Freshness {
        review_due_at: due.filter(|_| days.is_some() && last.is_some()),
        review_expired: days.is_some() && expired.unwrap_or(false),
        last_verified_at: last,
    };
    Ok(Some((freshness, override_days, disabled, default)))
}

/// 配信に出す鮮度。`default_days` は記事のコンテンツタイプの既定の確認期限（日数）。
pub async fn get(
    conn: &mut PgConnection,
    default_days: Option<u32>,
    entry_id: Uuid,
) -> Result<Option<Freshness>> {
    Ok(compute(conn, |_| default_days, entry_id)
        .await?
        .map(|(f, ..)| f))
}

/// 管理画面に出す鮮度と、確認済みの記録の履歴。
pub async fn detail(
    conn: &mut PgConnection,
    manifest: &Manifest,
    entry_id: Uuid,
) -> Result<Option<FreshnessDetail>> {
    let Some((freshness, review_interval_days, review_disabled, default_interval_days)) =
        compute(&mut *conn, |ct| default_interval(manifest, ct), entry_id).await?
    else {
        return Ok(None);
    };
    let history: Vec<(String, Option<Uuid>, String)> = sqlx::query_as(crate::sql!(
        "SELECT to_char(verified_at AT TIME ZONE 'UTC', {RFC3339}), verified_by, memo
         FROM verifications WHERE entry_id = $1 ORDER BY verified_at DESC, id LIMIT 100"
    ))
    .bind(entry_id)
    .fetch_all(conn)
    .await?;
    Ok(Some(FreshnessDetail {
        freshness,
        review_interval_days,
        review_disabled,
        default_interval_days,
        history: history
            .into_iter()
            .map(|(verified_at, verified_by, memo)| Verification {
                verified_at,
                verified_by,
                memo,
            })
            .collect(),
    }))
}

/// 「確認済み」を記録する（最終確認日を進める。版は作らない）。公開したことのない記事・削除済みの記事は false。
pub async fn verify(conn: &mut PgConnection, entry_id: Uuid, by: Uuid, memo: &str) -> Result<bool> {
    let updated = sqlx::query(
        "UPDATE entries SET last_verified_at = now()
         WHERE id = $1 AND deleted_at IS NULL AND EXISTS (SELECT 1 FROM versions WHERE entry_id = $1)",
    )
    .bind(entry_id)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if updated == 0 {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO verifications (id, entry_id, verified_by, memo) VALUES ($1, $2, $3, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(entry_id)
    .bind(by)
    .bind(memo)
    .execute(conn)
    .await?;
    Ok(true)
}

/// 確認期限の上書き。
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewOverride {
    /// 日数（null ならスキーマの既定に戻す）。
    #[serde(default)]
    pub interval_days: Option<i32>,
    /// 期限なしにする。
    #[serde(default)]
    pub disabled: bool,
}

/// 確認期限の上書きを保存する。削除済みなら false。
pub async fn set_review(
    conn: &mut PgConnection,
    entry_id: Uuid,
    r: ReviewOverride,
) -> Result<bool> {
    Ok(sqlx::query(
        "UPDATE entries SET review_interval_days = $2, review_disabled = $3 WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(entry_id)
    .bind(r.interval_days)
    .bind(r.disabled)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// 確認期限切れの公開中の記事（管理画面の一覧。期限の古い順）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Expired {
    pub entry_id: Uuid,
    pub locale: String,
    pub title: Option<String>,
    pub review_due_at: String,
}

pub async fn expired(conn: &mut PgConnection, manifest: &Manifest) -> Result<Vec<Expired>> {
    let mut out = Vec::new();
    for ct in &manifest.content_types {
        let default = ct.review.as_ref().map(|r| r.interval_days as i32);
        let rows: Vec<(Uuid, String, String, Option<String>)> = sqlx::query_as(crate::sql!(
            "WITH base AS (
                 SELECT e.id, e.locale, cur.fields,
                        COALESCE(e.last_verified_at, (SELECT min(published_at) FROM versions WHERE entry_id = e.id)) AS last,
                        CASE WHEN e.review_disabled THEN NULL ELSE COALESCE(e.review_interval_days, $2) END AS days
                 FROM entries e JOIN versions cur ON cur.id = e.published_version_id
                 WHERE e.content_type = $1 AND e.deleted_at IS NULL
             )
             SELECT id, locale, to_char((last + make_interval(days => days)) AT TIME ZONE 'UTC', {RFC3339}), fields::text
             FROM base WHERE days IS NOT NULL AND last + make_interval(days => days) < now()
             ORDER BY last + make_interval(days => days), id
             LIMIT 500"
        ))
        .bind(&ct.name)
        .bind(default)
        .fetch_all(&mut *conn)
        .await?;
        for (entry_id, locale, due, fields) in rows {
            let title = ct.title_field.as_deref().and_then(|f| {
                serde_json::from_str::<serde_json::Value>(fields.as_deref()?)
                    .ok()?
                    .get(f)?
                    .as_str()
                    .map(str::to_owned)
            });
            out.push(Expired {
                entry_id,
                locale,
                title,
                review_due_at: due,
            });
        }
    }
    out.sort_by(|a, b| a.review_due_at.cmp(&b.review_due_at));
    Ok(out)
}
