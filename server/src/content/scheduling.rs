//! 予約公開の保存（03-content-and-editing.md「予約公開（Q78）」、Q133、計画の9番）。ワークスペースのロールのトランザクションの中で呼ぶ。
//!
//! - 予約のときに公開の検査と計算を済ませた結果（予約版）を保存する。開いている予約は記事ごとに1つで、予約し直すと置き換える。
//! - 予約の時刻にジョブが予約版を公開する。公開できなければ理由を残してやり直し、予約の時刻から24時間たったら失敗にする。
//! - 訂正として公開・その場で公開・削除で取り消す（非公開では残す）。閉じたら予約版の中身は消す。

use anyhow::Result;
use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

use super::publishing::Prepared;

const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;

/// 予約の時刻から、やり直しをやめるまでの時間（秒）。
pub const GIVE_UP_AFTER_SECONDS: i64 = 24 * 60 * 60;
/// 予約できる時刻の上限（いまから、秒）。
pub const MAX_AHEAD_SECONDS: i64 = 366 * 24 * 60 * 60;

/// 予約を取り消した理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// 公開できる人が取り消した。
    Cancelled,
    /// 予約し直した（新しい予約に置き換えた）。
    Replaced,
    /// 訂正として公開した（Q133）。
    Corrected,
    /// その場で公開した。
    Published,
    /// 記事を削除した。
    Deleted,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Cancelled => "cancelled",
            Reason::Replaced => "replaced",
            Reason::Corrected => "corrected",
            Reason::Published => "published",
            Reason::Deleted => "deleted",
        }
    }
}

/// 予約の時刻の書き方を確かめる（RFC 3339。`2026-10-24T09:00:00Z`・`+09:00` など。小数の秒も可）。
/// 解釈と範囲の確認は [`check_time`] でDBに任せる。
pub fn well_formed_time(s: &str) -> bool {
    let b = s.as_bytes();
    let digits =
        |r: std::ops::Range<usize>| b.get(r).is_some_and(|d| d.iter().all(u8::is_ascii_digit));
    if b.len() < 20 || b.len() > 35 {
        return false;
    }
    let date_time = digits(0..4)
        && b[4] == b'-'
        && digits(5..7)
        && b[7] == b'-'
        && digits(8..10)
        && (b[10] == b'T' || b[10] == b't')
        && digits(11..13)
        && b[13] == b':'
        && digits(14..16)
        && b[16] == b':'
        && digits(17..19);
    if !date_time {
        return false;
    }
    let mut rest = &s[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        let n = frac.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 || n > 9 {
            return false;
        }
        rest = &frac[n..];
    }
    match rest.as_bytes() {
        [b'Z' | b'z'] => true,
        [b'+' | b'-', h1, h2, b':', m1, m2] => [h1, h2, m1, m2].iter().all(|d| d.is_ascii_digit()),
        _ => false,
    }
}

/// 予約の時刻を、DBで解釈して確かめた結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeCheck {
    Ok,
    /// 日時として成り立たない（2月30日など）。
    Invalid,
    /// いまより前か、上限より先。
    OutOfRange,
}

/// 予約の時刻を、DBの時計で確かめる（PostgreSQL 16 の pg_input_is_valid で、例外を出さずに解釈できるかを見る）。
/// 形の正しい時刻を渡す。
pub async fn check_time(conn: &mut PgConnection, publish_at: &str) -> Result<TimeCheck> {
    let valid: bool = sqlx::query_scalar("SELECT pg_input_is_valid($1, 'timestamptz')")
        .bind(publish_at)
        .fetch_one(&mut *conn)
        .await?;
    if !valid {
        return Ok(TimeCheck::Invalid);
    }
    let in_range: bool = sqlx::query_scalar(
        "SELECT $1::timestamptz > now() AND $1::timestamptz <= now() + make_interval(secs => $2)",
    )
    .bind(publish_at)
    .bind(MAX_AHEAD_SECONDS as f64)
    .fetch_one(conn)
    .await?;
    Ok(if in_range {
        TimeCheck::Ok
    } else {
        TimeCheck::OutOfRange
    })
}

/// 予約（予約版の中身を除く）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    pub id: Uuid,
    pub entry_id: Uuid,
    pub publish_at: String,
    pub draft_revision: i64,
    pub scheduled_by: Uuid,
    pub created_at: String,
    /// pending・published・cancelled・failed
    pub state: String,
    pub reason: Option<String>,
    /// 公開できなかった理由（やり直している間と、失敗したとき）。
    pub last_error: Option<String>,
    pub next_attempt_at: Option<String>,
    pub closed_at: Option<String>,
    pub closed_by: Option<Uuid>,
    /// 予約の後に下書きが編集されたか（開いている予約のときだけ意味がある）。
    pub draft_changed: bool,
}

type Row = (
    Uuid,
    Uuid,
    String,
    i64,
    Uuid,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<Uuid>,
    bool,
);

fn schedule(
    (
        id,
        entry_id,
        publish_at,
        draft_revision,
        scheduled_by,
        created_at,
        state,
        reason,
        last_error,
        next_attempt_at,
        closed_at,
        closed_by,
        draft_changed,
    ): Row,
) -> Schedule {
    Schedule {
        id,
        entry_id,
        publish_at,
        draft_revision,
        scheduled_by,
        created_at,
        state,
        reason,
        last_error,
        next_attempt_at,
        closed_at,
        closed_by,
        draft_changed,
    }
}

fn select(filter: &str, tail: &str) -> sqlx::AssertSqlSafe<String> {
    crate::sql!(
        "SELECT s.id, s.entry_id, to_char(s.publish_at AT TIME ZONE 'UTC', {RFC3339}), s.draft_revision,
                s.scheduled_by, to_char(s.created_at AT TIME ZONE 'UTC', {RFC3339}), s.state, s.reason,
                s.last_error, to_char(s.next_attempt_at AT TIME ZONE 'UTC', {RFC3339}),
                to_char(s.closed_at AT TIME ZONE 'UTC', {RFC3339}), s.closed_by,
                e.draft_revision > s.draft_revision
         FROM schedules s JOIN entries e ON e.id = s.entry_id
         WHERE {filter}
         {tail}"
    )
}

async fn by_id(conn: &mut PgConnection, id: Uuid) -> Result<Option<Schedule>> {
    let row: Option<Row> = sqlx::query_as(select("s.id = $1", ""))
        .bind(id)
        .fetch_optional(conn)
        .await?;
    Ok(row.map(schedule))
}

/// 記事の開いている予約。
pub async fn open(conn: &mut PgConnection, entry_id: Uuid) -> Result<Option<Schedule>> {
    let row: Option<Row> = sqlx::query_as(select("s.entry_id = $1 AND s.state = 'pending'", ""))
        .bind(entry_id)
        .fetch_optional(conn)
        .await?;
    Ok(row.map(schedule))
}

/// 記事の最後の予約（開いていても閉じていても）。
pub async fn latest(conn: &mut PgConnection, entry_id: Uuid) -> Result<Option<Schedule>> {
    let row: Option<Row> = sqlx::query_as(select(
        "s.entry_id = $1",
        "ORDER BY s.created_at DESC, s.id LIMIT 1",
    ))
    .bind(entry_id)
    .fetch_optional(conn)
    .await?;
    Ok(row.map(schedule))
}

/// 予約する（開いている予約があれば「予約し直し」として閉じる）。閉じた前の予約と、新しい予約を返す。
#[allow(clippy::too_many_arguments)]
pub async fn create(
    conn: &mut PgConnection,
    id: Uuid,
    entry_id: Uuid,
    publish_at: &str,
    draft_revision: i64,
    schema_version: i32,
    prepared: &Prepared,
    by: Uuid,
) -> Result<(Option<Schedule>, Schedule)> {
    let replaced = cancel(conn, entry_id, Reason::Replaced, by).await?;
    sqlx::query(
        "INSERT INTO schedules (id, entry_id, publish_at, draft_revision, schema_version, prepared, scheduled_by)
         VALUES ($1, $2, $3::timestamptz, $4, $5, $6::jsonb, $7)",
    )
    .bind(id)
    .bind(entry_id)
    .bind(publish_at)
    .bind(draft_revision)
    .bind(schema_version)
    .bind(serde_json::to_string(prepared)?)
    .bind(by)
    .execute(&mut *conn)
    .await?;
    let created = by_id(conn, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the schedule just created is missing"))?;
    Ok((replaced, created))
}

/// 開いている予約を取り消す。取り消した予約を返す（なければ None）。
pub async fn cancel(
    conn: &mut PgConnection,
    entry_id: Uuid,
    reason: Reason,
    by: Uuid,
) -> Result<Option<Schedule>> {
    let id: Option<Uuid> = sqlx::query_scalar(
        "UPDATE schedules SET state = 'cancelled', reason = $2, prepared = NULL, closed_at = now(), closed_by = $3
         WHERE entry_id = $1 AND state = 'pending'
         RETURNING id",
    )
    .bind(entry_id)
    .bind(reason.as_str())
    .bind(by)
    .fetch_optional(&mut *conn)
    .await?;
    match id {
        Some(id) => by_id(conn, id).await,
        None => Ok(None),
    }
}

/// 公開する予約（開いていて、時刻を過ぎたもの）を、予約版と一緒にロックして読む。
/// (記事のID, テンプレートの版, 予約した人, 予約版, 予約の時刻から経った秒数)
pub async fn take_due(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<(Uuid, i32, Uuid, Prepared, i64)>> {
    let row: Option<(Uuid, i32, Uuid, Option<String>, f64)> = sqlx::query_as(
        "SELECT entry_id, schema_version, scheduled_by, prepared::text,
                extract(epoch FROM now() - publish_at)::float8
         FROM schedules WHERE id = $1 AND state = 'pending' AND publish_at <= now()
         FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((entry_id, schema_version, by, Some(prepared), elapsed)) = row else {
        return Ok(None);
    };
    Ok(Some((
        entry_id,
        schema_version,
        by,
        serde_json::from_str(&prepared)?,
        elapsed as i64,
    )))
}

/// 予約版を公開した。
pub async fn mark_published(conn: &mut PgConnection, id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE schedules SET state = 'published', prepared = NULL, closed_at = now(), last_error = NULL,
                              next_attempt_at = NULL
         WHERE id = $1",
    )
    .bind(id)
    .execute(conn)
    .await?;
    Ok(())
}

/// 公開できなかった（やり直す）。
pub async fn mark_retry(
    conn: &mut PgConnection,
    id: Uuid,
    error: &str,
    retry_in_seconds: f64,
) -> Result<()> {
    sqlx::query(
        "UPDATE schedules SET last_error = $2, attempts = attempts + 1,
                              next_attempt_at = now() + make_interval(secs => $3)
         WHERE id = $1 AND state = 'pending'",
    )
    .bind(id)
    .bind(error)
    .bind(retry_in_seconds)
    .execute(conn)
    .await?;
    Ok(())
}

/// やり直しをやめた（失敗）。失敗にした予約を返す。
pub async fn mark_failed(
    conn: &mut PgConnection,
    id: Uuid,
    error: &str,
) -> Result<Option<Schedule>> {
    let done = sqlx::query(
        "UPDATE schedules SET state = 'failed', last_error = $2, prepared = NULL, closed_at = now(),
                              next_attempt_at = NULL
         WHERE id = $1 AND state = 'pending'",
    )
    .bind(id)
    .bind(error)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() != 1 {
        return Ok(None);
    }
    by_id(conn, id).await
}

/// やり直している予約（削除していない記事だけ。予約の時刻の古い順）。
pub async fn retrying(conn: &mut PgConnection) -> Result<Vec<Schedule>> {
    let rows: Vec<Row> = sqlx::query_as(select(
        "s.state = 'pending' AND s.last_error IS NOT NULL AND e.deleted_at IS NULL",
        "ORDER BY s.publish_at, s.id LIMIT 500",
    ))
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(schedule).collect())
}

/// 利用者が予約して、訂正による取り消しか失敗で閉じたもの（記事ごとに最後の予約がそれで、30日以内のもの。新しい順）。
pub async fn needing_attention(conn: &mut PgConnection, user: Uuid) -> Result<Vec<Schedule>> {
    let rows: Vec<Row> = sqlx::query_as(select(
        "s.scheduled_by = $1 AND e.deleted_at IS NULL
         AND (s.state = 'failed' OR (s.state = 'cancelled' AND s.reason = 'corrected'))
         AND s.closed_at > now() - interval '30 days'
         AND NOT EXISTS (SELECT 1 FROM schedules later
                         WHERE later.entry_id = s.entry_id AND later.created_at > s.created_at)",
        "ORDER BY s.closed_at DESC, s.id LIMIT 500",
    ))
    .bind(user)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(schedule).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_must_be_rfc3339() {
        for ok in [
            "2026-10-24T09:00:00Z",
            "2026-10-24T09:00:00.123Z",
            "2026-10-24T18:00:00+09:00",
            "2026-10-24t09:00:00z",
        ] {
            assert!(well_formed_time(ok), "{ok}");
        }
        for bad in [
            "",
            "2026-10-24",
            "2026-10-24 09:00:00Z",
            "2026-10-24T09:00:00",
            "2026-10-24T09:00:00+0900",
            "2026-10-24T09:00:00.Z",
            "now",
        ] {
            assert!(!well_formed_time(bad), "{bad}");
        }
    }
}
