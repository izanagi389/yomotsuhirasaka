//! 登録済みクエリ（Persisted Queries。13-security.md「GraphQL」、Q94, Q151）。
//! ワークスペースのロールに切り替えたトランザクションの中で呼ぶ。
//!
//! クエリのIDは、クエリの文のSHA-256（16進数64桁）。同じ文を登録し直すと、最後に使った日時だけが進む。

use anyhow::Result;
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use uuid::Uuid;

/// クエリの文の大きさの上限（バイト）。
pub const MAX_QUERY_BYTES: usize = 32 * 1024;

/// クエリのID。
pub fn id_of(query: &str) -> String {
    Sha256::digest(query.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// IDの形が正しいか（16進数の小文字64桁）。
pub fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// 登録する（同じ文がもうあれば、最後に使った日時を進める）。
pub async fn register(conn: &mut PgConnection, query: &str, key_id: Uuid) -> Result<String> {
    let id = id_of(query);
    sqlx::query(
        "INSERT INTO queries (id, query, registered_by) VALUES ($1, $2, $3)
         ON CONFLICT (id) DO UPDATE SET last_used_at = now()",
    )
    .bind(&id)
    .bind(query)
    .bind(key_id)
    .execute(conn)
    .await?;
    Ok(id)
}

/// 登録済みのクエリの文（なければ None）。使った日時を記録する。
pub async fn lookup(conn: &mut PgConnection, id: &str) -> Result<Option<String>> {
    if !valid_id(id) {
        return Ok(None);
    }
    // 使った日時は1分に1回だけ書く
    let touched: Option<String> = sqlx::query_scalar(
        "UPDATE queries SET last_used_at = now()
         WHERE id = $1 AND last_used_at < now() - interval '1 minute'
         RETURNING query",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    if touched.is_some() {
        return Ok(touched);
    }
    Ok(
        sqlx::query_scalar("SELECT query FROM queries WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        let id = id_of("{ a }");
        assert!(valid_id(&id));
        assert_eq!(id, id_of("{ a }"));
        assert_ne!(id, id_of("{ b }"));
        assert!(!valid_id(&id.to_uppercase()) && !valid_id("abc"));
    }
}
