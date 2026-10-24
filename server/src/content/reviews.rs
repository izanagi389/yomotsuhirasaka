//! レビュー依頼（03-content-and-editing.md「レビュー（Q77）」、計画の8番）。ワークスペースのロールのトランザクションの中で呼ぶ。
//!
//! - 1段階だけ: 依頼 → 差し戻し（コメント必須）・取り下げ・公開・予約公開のどれかで閉じる。承認という段階は作らない。
//! - 開いている依頼は記事ごとに1つ。依頼した時点の下書きのリビジョンを残す（その後の編集で依頼は消えない）。
//! - 見てほしい人（reviewer）は目印だけ。だれが差し戻し・公開できるかは、HTTPの層でロールを照合する。

use anyhow::Result;
use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

/// 差し戻しのコメントの長さの上限（文字数）。
pub const MAX_COMMENT_CHARS: usize = 2000;
/// 記事ごとの履歴の件数の上限。
const HISTORY_LIMIT: i64 = 100;

const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;

/// 依頼の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Open,
    Rejected,
    Withdrawn,
    Published,
    /// 予約公開した（予約した時点で閉じる）。
    Scheduled,
}

impl State {
    fn as_str(self) -> &'static str {
        match self {
            State::Open => "open",
            State::Rejected => "rejected",
            State::Withdrawn => "withdrawn",
            State::Published => "published",
            State::Scheduled => "scheduled",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "rejected" => State::Rejected,
            "withdrawn" => State::Withdrawn,
            "published" => State::Published,
            "scheduled" => State::Scheduled,
            _ => State::Open,
        }
    }
}

/// レビュー依頼の1件。
#[derive(Debug, Clone)]
pub struct Request {
    pub id: Uuid,
    pub entry_id: Uuid,
    pub requested_by: Uuid,
    pub requested_revision: i64,
    pub reviewer: Option<Uuid>,
    pub state: State,
    pub comment: Option<String>,
    pub created_at: String,
    pub closed_at: Option<String>,
    pub closed_by: Option<Uuid>,
    /// 依頼の後に下書きが編集されたか（開いている依頼のときだけ意味がある）。
    pub changed_since_request: bool,
}

type Row = (
    Uuid,
    Uuid,
    Uuid,
    i64,
    Option<Uuid>,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<Uuid>,
    bool,
);

fn request(
    (
        id,
        entry_id,
        requested_by,
        requested_revision,
        reviewer,
        state,
        comment,
        created_at,
        closed_at,
        closed_by,
        changed,
    ): Row,
) -> Request {
    Request {
        id,
        entry_id,
        requested_by,
        requested_revision,
        reviewer,
        state: State::parse(&state),
        comment,
        created_at,
        closed_at,
        closed_by,
        changed_since_request: changed,
    }
}

fn select(filter: &str, tail: &str) -> sqlx::AssertSqlSafe<String> {
    crate::sql!(
        "SELECT r.id, r.entry_id, r.requested_by, r.requested_revision, r.reviewer, r.state, r.comment,
                to_char(r.created_at AT TIME ZONE 'UTC', {RFC3339}),
                to_char(r.closed_at AT TIME ZONE 'UTC', {RFC3339}),
                r.closed_by, e.draft_revision > r.requested_revision
         FROM review_requests r JOIN entries e ON e.id = r.entry_id
         WHERE {filter}
         {tail}"
    )
}

/// 差し戻しのコメントを、保存する形（前後の空白を除く）にする。空・長すぎる・改行とタブ以外の制御文字を含むものは None。
pub fn normalize_comment(raw: &str) -> Option<String> {
    let c = raw.trim();
    let ok = !c.is_empty()
        && c.chars().count() <= MAX_COMMENT_CHARS
        && !c
            .chars()
            .any(|ch| ch.is_control() && ch != '\n' && ch != '\t');
    ok.then(|| c.to_owned())
}

/// 記事の開いている依頼。
pub async fn open(conn: &mut PgConnection, entry_id: Uuid) -> Result<Option<Request>> {
    let row: Option<Row> = sqlx::query_as(select("r.entry_id = $1 AND r.state = 'open'", ""))
        .bind(entry_id)
        .fetch_optional(conn)
        .await?;
    Ok(row.map(request))
}

/// 記事の依頼の履歴（新しい順）。
pub async fn history(conn: &mut PgConnection, entry_id: Uuid) -> Result<Vec<Request>> {
    let rows: Vec<Row> = sqlx::query_as(select(
        "r.entry_id = $1",
        "ORDER BY r.created_at DESC, r.id LIMIT $2",
    ))
    .bind(entry_id)
    .bind(HISTORY_LIMIT)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(request).collect())
}

/// 依頼を作れなかった理由。
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// 記事がない（削除した記事を含む）。
    NotFound,
    /// すでに開いている依頼がある。
    AlreadyOpen,
}

/// 依頼する。いまの下書きのリビジョンを記録する。
pub async fn create(
    conn: &mut PgConnection,
    entry_id: Uuid,
    by: Uuid,
    reviewer: Option<Uuid>,
) -> Result<Result<Request, Refused>> {
    let revision: Option<i64> = sqlx::query_scalar(
        "SELECT draft_revision FROM entries WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(entry_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(revision) = revision else {
        return Ok(Err(Refused::NotFound));
    };
    let id = Uuid::new_v4();
    let inserted = sqlx::query(
        "INSERT INTO review_requests (id, entry_id, requested_by, requested_revision, reviewer)
         VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(entry_id)
    .bind(by)
    .bind(revision)
    .bind(reviewer)
    .execute(&mut *conn)
    .await?;
    if inserted.rows_affected() != 1 {
        return Ok(Err(Refused::AlreadyOpen));
    }
    Ok(open(conn, entry_id).await?.ok_or(Refused::NotFound))
}

/// 開いている依頼の、見てほしい人を変える（None で外す）。開いている依頼がなければ None。
pub async fn set_reviewer(
    conn: &mut PgConnection,
    entry_id: Uuid,
    reviewer: Option<Uuid>,
) -> Result<Option<Request>> {
    let done = sqlx::query(
        "UPDATE review_requests SET reviewer = $2 WHERE entry_id = $1 AND state = 'open'",
    )
    .bind(entry_id)
    .bind(reviewer)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() != 1 {
        return Ok(None);
    }
    open(conn, entry_id).await
}

/// 開いている依頼を閉じる（差し戻しはコメント付き）。閉じた依頼を返す。開いている依頼がなければ None。
pub async fn close(
    conn: &mut PgConnection,
    entry_id: Uuid,
    state: State,
    by: Uuid,
    comment: Option<&str>,
) -> Result<Option<Request>> {
    debug_assert!(state != State::Open);
    let id: Option<Uuid> = sqlx::query_scalar(
        "UPDATE review_requests SET state = $2, comment = $3, closed_at = now(), closed_by = $4
         WHERE entry_id = $1 AND state = 'open'
         RETURNING id",
    )
    .bind(entry_id)
    .bind(state.as_str())
    .bind(comment)
    .bind(by)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(id) = id else {
        return Ok(None);
    };
    let row: Row = sqlx::query_as(select("r.id = $1", ""))
        .bind(id)
        .fetch_one(conn)
        .await?;
    Ok(Some(request(row)))
}

/// レビュー待ちの依頼（削除していない記事だけ。古い順）。
pub async fn waiting(conn: &mut PgConnection) -> Result<Vec<Request>> {
    let rows: Vec<Row> = sqlx::query_as(select(
        "r.state = 'open' AND e.deleted_at IS NULL",
        "ORDER BY r.created_at, r.id LIMIT 500",
    ))
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(request).collect())
}

/// 利用者が依頼して差し戻されたままの依頼（記事ごとに最新の依頼が差し戻しのもの。新しい順）。
pub async fn rejected_for(conn: &mut PgConnection, user: Uuid) -> Result<Vec<Request>> {
    let rows: Vec<Row> = sqlx::query_as(select(
        "r.state = 'rejected' AND r.requested_by = $1 AND e.deleted_at IS NULL
         AND NOT EXISTS (SELECT 1 FROM review_requests later
                         WHERE later.entry_id = r.entry_id AND later.created_at > r.created_at)",
        "ORDER BY r.closed_at DESC, r.id LIMIT 500",
    ))
    .bind(user)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(request).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_are_trimmed_and_checked() {
        assert_eq!(
            normalize_comment("  直してください\n2段落目 ").as_deref(),
            Some("直してください\n2段落目")
        );
        assert_eq!(
            normalize_comment(&"あ".repeat(MAX_COMMENT_CHARS)).map(|c| c.chars().count()),
            Some(MAX_COMMENT_CHARS)
        );
        for bad in ["", "  \n ", "a\u{7}b"] {
            assert_eq!(normalize_comment(bad), None, "{bad:?}");
        }
        assert_eq!(normalize_comment(&"あ".repeat(MAX_COMMENT_CHARS + 1)), None);
    }

    #[test]
    fn states_round_trip() {
        for s in [
            State::Open,
            State::Rejected,
            State::Withdrawn,
            State::Published,
            State::Scheduled,
        ] {
            assert_eq!(State::parse(s.as_str()), s);
        }
    }
}
