//! 編集のロック（ソフトロック。03-content-and-editing.md「同時編集（Q73）」、計画の10番）。
//! ワークスペースのロールのトランザクションの中で呼ぶ。
//!
//! - ロケールごとの記事に1つ。記事の画面（タブ）ごとのIDで持ち、生存の知らせで期限を延ばす。期限が過ぎたら無いものとして扱う。
//! - 下書きの保存は、ほかの画面の有効なロックがあれば断る（INV-LOCK-01）。保存ではロックを取らない（INV-EDIT-01 の流れを保つ）。

use anyhow::Result;
use sqlx::PgConnection;
use uuid::Uuid;

const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;

/// ロックの有効な長さ（秒）。画面は、その3分の1ごとに生存を知らせる。
pub const LEASE_SECONDS: f64 = 60.0;

/// 有効なロック。
#[derive(Debug, Clone)]
pub struct Lock {
    pub lock_id: Uuid,
    pub user_id: Uuid,
    pub session_id: Uuid,
    pub acquired_at: String,
    pub expires_at: String,
}

type Row = (Uuid, Uuid, Uuid, String, String);

fn lock((lock_id, user_id, session_id, acquired_at, expires_at): Row) -> Lock {
    Lock {
        lock_id,
        user_id,
        session_id,
        acquired_at,
        expires_at,
    }
}

/// 記事の有効なロック（期限の過ぎたものは無いものとして扱う）。
pub async fn current(conn: &mut PgConnection, entry_id: Uuid) -> Result<Option<Lock>> {
    let row: Option<Row> = sqlx::query_as(crate::sql!(
        "SELECT lock_id, user_id, session_id, to_char(acquired_at AT TIME ZONE 'UTC', {RFC3339}),
                to_char(expires_at AT TIME ZONE 'UTC', {RFC3339})
         FROM edit_locks WHERE entry_id = $1 AND expires_at > now()"
    ))
    .bind(entry_id)
    .fetch_optional(conn)
    .await?;
    Ok(row.map(lock))
}

/// ロックを取れなかった理由。
#[derive(Debug)]
pub enum Refused {
    /// 記事がない（削除済みを含む）。
    NotFound,
    /// ほかの画面が有効なロックを持っている。
    Held(Lock),
}

/// 記事の行をロックして、あるか確かめる（同時のロックの取得を順番にする）。
async fn lock_entry(conn: &mut PgConnection, entry_id: Uuid) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM entries WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(entry_id)
    .fetch_optional(conn)
    .await?
    .is_some())
}

/// 新しいロックを書く（前のロックがあれば置き換える）。
async fn put(
    conn: &mut PgConnection,
    entry_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<Lock> {
    let row: Row = sqlx::query_as(crate::sql!(
        "INSERT INTO edit_locks (entry_id, lock_id, user_id, session_id, expires_at)
         VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5))
         ON CONFLICT (entry_id) DO UPDATE
             SET lock_id = EXCLUDED.lock_id, user_id = EXCLUDED.user_id, session_id = EXCLUDED.session_id,
                 acquired_at = now(), expires_at = EXCLUDED.expires_at
         RETURNING lock_id, user_id, session_id, to_char(acquired_at AT TIME ZONE 'UTC', {RFC3339}),
                   to_char(expires_at AT TIME ZONE 'UTC', {RFC3339})"
    ))
    .bind(entry_id)
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(session_id)
    .bind(LEASE_SECONDS)
    .fetch_one(conn)
    .await?;
    Ok(lock(row))
}

/// ロックを取る（記事の画面を開いたとき）。ほかの画面が有効なロックを持っていれば断る。
pub async fn acquire(
    conn: &mut PgConnection,
    entry_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Lock, Refused>> {
    if !lock_entry(conn, entry_id).await? {
        return Ok(Err(Refused::NotFound));
    }
    if let Some(held) = current(conn, entry_id).await? {
        return Ok(Err(Refused::Held(held)));
    }
    Ok(Ok(put(conn, entry_id, user_id, session_id).await?))
}

/// 編集を引き継ぐ（ほかの画面のロックを奪う）。新しいロックと、奪ったロック（あれば）を返す。
pub async fn take_over(
    conn: &mut PgConnection,
    entry_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<Option<(Lock, Option<Lock>)>> {
    if !lock_entry(conn, entry_id).await? {
        return Ok(None);
    }
    let previous = current(conn, entry_id).await?;
    let lock = put(conn, entry_id, user_id, session_id).await?;
    Ok(Some((lock, previous)))
}

/// 生存の知らせ。いまも自分のロックなら期限を延ばして返す（奪われていたり、期限が過ぎていたりすれば None）。
pub async fn heartbeat(
    conn: &mut PgConnection,
    entry_id: Uuid,
    lock_id: Uuid,
) -> Result<Option<Lock>> {
    let row: Option<Row> = sqlx::query_as(crate::sql!(
        "UPDATE edit_locks SET expires_at = now() + make_interval(secs => $3)
         WHERE entry_id = $1 AND lock_id = $2 AND expires_at > now()
         RETURNING lock_id, user_id, session_id, to_char(acquired_at AT TIME ZONE 'UTC', {RFC3339}),
                   to_char(expires_at AT TIME ZONE 'UTC', {RFC3339})"
    ))
    .bind(entry_id)
    .bind(lock_id)
    .bind(LEASE_SECONDS)
    .fetch_optional(conn)
    .await?;
    Ok(row.map(lock))
}

/// ロックを外す（画面を離れたとき）。自分のロックだったら true。
pub async fn release(conn: &mut PgConnection, entry_id: Uuid, lock_id: Uuid) -> Result<bool> {
    let done = sqlx::query("DELETE FROM edit_locks WHERE entry_id = $1 AND lock_id = $2")
        .bind(entry_id)
        .bind(lock_id)
        .execute(conn)
        .await?;
    Ok(done.rows_affected() == 1)
}

/// 下書きを保存してよいか（INV-LOCK-01）。ほかの画面の有効なロックがあれば、そのロックを返す。
/// - ロックのIDを付けた保存: 有効なロックがそのIDか、有効なロックがなければよい
/// - ロックのIDを付けない保存（APIから直接など）: 有効なロックがなければよい
pub async fn blocking_save(
    conn: &mut PgConnection,
    entry_id: Uuid,
    lock_id: Option<Uuid>,
) -> Result<Option<Lock>> {
    Ok(current(conn, entry_id)
        .await?
        .filter(|held| Some(held.lock_id) != lock_id))
}
