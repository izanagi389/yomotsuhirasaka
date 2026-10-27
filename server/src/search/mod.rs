//! サイト内検索（08-search-and-related.md、M3）。
//!
//! - 検索用データ（チャンクと埋め込み）は、ワークスペースのスキーマの `search_chunks` に置く。作るのは [`index`] のジョブ。
//! - ベクトルの比べ方には pgvector を使う。拡張はスーパーユーザーが前もって作る（CMSは作らない。ロールがスーパーユーザーでないため）。
//!   ワークスペースの SQL は search_path がそのワークスペースのスキーマだけなので、型と演算子は拡張のスキーマの名前を付けて書く。

pub mod chunk;
pub mod index;

use anyhow::{Result, bail};
use sqlx::PgExecutor;

use crate::db::Ident;

/// pgvector の拡張 `vector` のあるスキーマ（なければ None）。
pub async fn vector_schema<'e>(executor: impl PgExecutor<'e>) -> Result<Option<Ident>> {
    let name: Option<String> = sqlx::query_scalar(
        "SELECT n.nspname FROM pg_extension e JOIN pg_namespace n ON n.oid = e.extnamespace
         WHERE e.extname = 'vector'",
    )
    .fetch_optional(executor)
    .await?;
    match name {
        None => Ok(None),
        Some(n) => match Ident::new(&n) {
            Ok(i) => Ok(Some(i)),
            Err(_) => bail!("the schema of the extension `vector` has an unsupported name"),
        },
    }
}

/// 拡張のスキーマを、どのロール（ワークスペースのロールを含む）も使えるか（PUBLIC に USAGE があるか）。
pub async fn usable_by_every_role<'e>(
    executor: impl PgExecutor<'e>,
    schema: &Ident,
) -> Result<bool> {
    let ok: Option<bool> = sqlx::query_scalar(
        "SELECT bool_or(a.grantee = 0 AND a.privilege_type = 'USAGE')
         FROM pg_namespace n, aclexplode(coalesce(n.nspacl, acldefault('n', n.nspowner))) a
         WHERE n.nspname = $1",
    )
    .bind(schema.as_str())
    .fetch_one(executor)
    .await?;
    Ok(ok.unwrap_or(false))
}
