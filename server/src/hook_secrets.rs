//! フックが読む秘密（04-hooks.md「秘密」）。
//!
//! - 関数のソースはリポジトリとDBに平文で残るので、外部のAPIキーなどはここに置く。
//! - 値は暗号化して保存し、AADにワークスペースIDと名前を入れる。登録の後は、管理APIでも値を返さない。
//! - 読めるのは非同期のフックだけで、権限 `secrets.read` と、使う名前の宣言（`secrets`）が要る。
//!   宣言した名前だけを、実行の前に復号して渡す。

use std::collections::HashMap;

use anyhow::Result;
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::keys::KeyRing;

/// ワークスペースごとの数の上限。
pub const MAX_SECRETS: i64 = 50;
/// 値の大きさの上限（バイト）。
pub const MAX_VALUE_BYTES: usize = 4096;
/// 名前の長さの上限。
pub const MAX_NAME_CHARS: usize = 64;

const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;

/// 名前として正しいか（英字で始まる、英数字と `_`）。
pub fn valid_name(name: &str) -> bool {
    name.len() <= MAX_NAME_CHARS
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// 暗号化のAAD。
pub fn aad(workspace_id: Uuid, name: &str) -> Vec<u8> {
    format!("yomi:hook-secret:{workspace_id}:{name}").into_bytes()
}

/// 一覧の1件（値は含めない）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub name: String,
    pub updated_at: String,
}

pub async fn list(pool: &PgPool, workspace_id: Uuid) -> Result<Vec<Summary>> {
    let rows: Vec<(String, String)> = sqlx::query_as(crate::sql!(
        "SELECT name, to_char(updated_at AT TIME ZONE 'UTC', {RFC3339})
         FROM yomi.hook_secrets WHERE workspace_id = $1 ORDER BY name"
    ))
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(name, updated_at)| Summary { name, updated_at })
        .collect())
}

/// 登録か置き換えの結果。
#[derive(Debug, PartialEq, Eq)]
pub enum Saved {
    Done,
    TooMany,
}

/// 登録するか、同じ名前の値を置き換える。監査ログと同じトランザクションで呼ぶ。
pub async fn save(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    keys: &KeyRing,
    workspace_id: Uuid,
    name: &str,
    value: &str,
    actor: Uuid,
) -> Result<Saved> {
    // 同時の登録で上限を超えないよう、ワークスペースごとに1つずつ行う
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('yomi:hook-secrets:' || $1::text, 0))",
    )
    .bind(workspace_id)
    .execute(&mut **tx)
    .await?;
    let (others, exists): (i64, bool) = sqlx::query_as(
        "SELECT count(*), coalesce(bool_or(name = $2), false) FROM yomi.hook_secrets WHERE workspace_id = $1",
    )
    .bind(workspace_id)
    .bind(name)
    .fetch_one(&mut **tx)
    .await?;
    if !exists && others >= MAX_SECRETS {
        return Ok(Saved::TooMany);
    }
    let sealed = keys.encrypt(value.as_bytes(), &aad(workspace_id, name))?;
    sqlx::query(
        "INSERT INTO yomi.hook_secrets (workspace_id, name, value_enc, updated_by)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (workspace_id, name)
         DO UPDATE SET value_enc = $3, updated_by = $4, updated_at = now()",
    )
    .bind(workspace_id)
    .bind(name)
    .bind(sealed)
    .bind(actor)
    .execute(&mut **tx)
    .await?;
    Ok(Saved::Done)
}

/// 消す。あれば true。
pub async fn delete(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
    name: &str,
) -> Result<bool> {
    Ok(
        sqlx::query("DELETE FROM yomi.hook_secrets WHERE workspace_id = $1 AND name = $2")
            .bind(workspace_id)
            .bind(name)
            .execute(&mut **tx)
            .await?
            .rows_affected()
            == 1,
    )
}

/// 宣言した名前の値を復号する（登録のない名前は入れない）。
pub async fn load(
    pool: &PgPool,
    keys: &KeyRing,
    workspace_id: Uuid,
    names: &[String],
) -> Result<HashMap<String, String>> {
    if names.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(String, Vec<u8>)> = sqlx::query_as(
        "SELECT name, value_enc FROM yomi.hook_secrets WHERE workspace_id = $1 AND name = ANY($2)",
    )
    .bind(workspace_id)
    .bind(names)
    .fetch_all(pool)
    .await?;
    let mut out = HashMap::with_capacity(rows.len());
    for (name, sealed) in rows {
        let plain = keys.decrypt(&sealed, &aad(workspace_id, &name))?;
        out.insert(name, String::from_utf8(plain)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for good in ["A", "API_KEY", "key1"] {
            assert!(valid_name(good), "{good}");
        }
        for bad in ["", "1a", "_a", "a-b", "a b", "鍵", &"a".repeat(65)] {
            assert!(!valid_name(bad), "{bad}");
        }
    }
}
