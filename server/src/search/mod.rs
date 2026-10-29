//! サイト内検索（08-search-and-related.md、M3）。
//!
//! - 検索用データ（チャンクと埋め込み）は、ワークスペースのスキーマの `search_chunks` に置く。作るのは [`index`] のジョブ。
//! - ベクトルの比べ方には pgvector を使う。拡張はスーパーユーザーが前もって作る（CMSは作らない。ロールがスーパーユーザーでないため）。
//!   ワークスペースの SQL は search_path がそのワークスペースのスキーマだけなので、型と演算子は拡張のスキーマの名前を付けて書く。
//! - 日本語のキーワード検索には pg_bigm を使う（M3。PGroonga は使わない）。これもスーパーユーザーが前もって作る。
//!   なければ、キーワード検索だけを無効にする（INV-SRC-05）。あれば、CMSが移行用の接続で、各ワークスペースの本文に
//!   GIN の索引（gin_bigm_ops）を作る（起動のときと、ワークスペースを作ったとき）。
//! - 埋め込みには、ワークスペース・モデルごとに HNSW の部分索引を作る（[`ensure_vector_index`]。これも移行用の接続で、
//!   起動のときと、ワークスペースを作ったとき）。

pub mod access;
pub mod chunk;
pub mod index;
pub mod query;
pub mod related;
pub mod switch;

use anyhow::{Result, bail};
use sha2::{Digest as _, Sha256};
use sqlx::{PgConnection, PgExecutor, PgPool};
use uuid::Uuid;

use crate::config::Config;
use crate::db::Ident;
use crate::workspace_db::WorkspaceDb;

/// 拡張のあるスキーマ（なければ None）。
async fn extension_schema<'e>(
    executor: impl PgExecutor<'e>,
    extension: &str,
) -> Result<Option<Ident>> {
    let name: Option<String> = sqlx::query_scalar(
        "SELECT n.nspname FROM pg_extension e JOIN pg_namespace n ON n.oid = e.extnamespace
         WHERE e.extname = $1",
    )
    .bind(extension)
    .fetch_optional(executor)
    .await?;
    match name {
        None => Ok(None),
        Some(n) => match Ident::new(&n) {
            Ok(i) => Ok(Some(i)),
            Err(_) => bail!("the schema of the extension `{extension}` has an unsupported name"),
        },
    }
}

/// pgvector の拡張 `vector` のあるスキーマ（なければ None）。
pub async fn vector_schema<'e>(executor: impl PgExecutor<'e>) -> Result<Option<Ident>> {
    extension_schema(executor, "vector").await
}

/// pg_bigm の拡張 `pg_bigm` のあるスキーマ。拡張がない、またはそのスキーマをどのロールも使えるのでなければ None
/// （キーワード検索は無効）。
pub async fn keyword_schema(conn: &mut PgConnection) -> Result<Option<Ident>> {
    let Some(schema) = extension_schema(&mut *conn, "pg_bigm").await? else {
        return Ok(None);
    };
    Ok(usable_by_every_role(&mut *conn, &schema)
        .await?
        .then_some(schema))
}

/// pg_bigm があれば、ワークスペースのチャンクの本文に GIN の索引を作る（既にあれば何もしない）。移行・ロールの作成用の接続で呼ぶ。
/// 大文字・小文字を区別せずに探すため、索引は `lower(content)` に作る（検索の側も同じ式を使う。`query.rs`）。
pub async fn ensure_keyword_index(conn: &mut PgConnection, db: &WorkspaceDb) -> Result<()> {
    let Some(bs) = keyword_schema(&mut *conn).await? else {
        return Ok(());
    };
    let schema = &db.schema;
    sqlx::query(crate::sql!(
        "CREATE INDEX IF NOT EXISTS search_chunks_bigm ON {schema}.search_chunks
         USING gin (lower(content) {bs}.gin_bigm_ops)"
    ))
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// すべてのワークスペースに [`ensure_keyword_index`] を行う（サーバーの起動時。後から pg_bigm を入れた環境のため）。
pub async fn ensure_keyword_indexes(migration: &PgPool) -> Result<()> {
    let mut conn = migration.acquire().await?;
    if keyword_schema(&mut conn).await?.is_none() {
        return Ok(());
    }
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM cms.workspaces ORDER BY created_at")
        .fetch_all(&mut *conn)
        .await?;
    for id in ids {
        ensure_keyword_index(&mut conn, &WorkspaceDb::names_for(id)).await?;
    }
    Ok(())
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

/// 埋め込みを作る・検索に使うモデルと次元数（設定の今のモデルと、切り替え先のモデル）。
pub fn embedding_models(config: &Config) -> Vec<(String, u32)> {
    let Some(llm) = &config.llm else {
        return Vec::new();
    };
    [Some(llm.current()), llm.next()]
        .into_iter()
        .flatten()
        .map(|l| (l.embedding_model, l.embedding_dimensions))
        .collect()
}

/// モデルと次元数ごとの HNSW の索引の名前（モデル名は任意の文字列なので、要約から作る）。
fn vector_index_name(model: &str, dimensions: u32) -> Ident {
    let digest = Sha256::new()
        .chain_update(model.as_bytes())
        .chain_update(dimensions.to_be_bytes())
        .finalize();
    let hex: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    Ident::new(&format!("search_embeddings_hnsw_{hex}")).expect("hex is a valid identifier")
}

/// 設定のモデルごとに、ワークスペースの埋め込みの HNSW の部分索引を作る（既にあれば何もしない）。移行・ロールの作成用の接続で呼ぶ。
/// 索引の式は `(embedding::vector(次元数)) vector_cosine_ops`、条件はモデルと次元数（検索の側も同じ式を使う。`query.rs`）。
/// 設定にないモデルの索引は消す（そのモデルでは検索しないため。埋め込みそのものは、切り替えの決まりでジョブが消す）。
/// LLMの接続先がない・pgvector がないときは何もしない。
pub async fn ensure_vector_index(
    conn: &mut PgConnection,
    db: &WorkspaceDb,
    config: &Config,
) -> Result<()> {
    let models = embedding_models(config);
    if models.is_empty() {
        return Ok(());
    }
    let Some(vs) = vector_schema(&mut *conn).await? else {
        return Ok(());
    };
    let schema = &db.schema;
    let wanted: Vec<Ident> = models
        .iter()
        .map(|(m, d)| vector_index_name(m, *d))
        .collect();
    let existing: Vec<String> = sqlx::query_scalar(
        "SELECT indexname::text FROM pg_indexes
         WHERE schemaname = $1 AND tablename = 'search_embeddings' AND indexname LIKE 'search\\_embeddings\\_hnsw\\_%'",
    )
    .bind(schema.as_str())
    .fetch_all(&mut *conn)
    .await?;
    for name in existing {
        let name = Ident::new(&name)?;
        if !wanted.contains(&name) {
            sqlx::query(crate::sql!("DROP INDEX IF EXISTS {schema}.{name}"))
                .execute(&mut *conn)
                .await?;
        }
    }
    for (model, d) in models {
        let name = vector_index_name(&model, d);
        // モデル名は文字列の定数として書く（索引の条件には、パラメーターを使えない）。引用は PostgreSQL の quote_literal に任せる
        let literal: String = sqlx::query_scalar("SELECT quote_literal($1::text)")
            .bind(&model)
            .fetch_one(&mut *conn)
            .await?;
        sqlx::query(crate::sql!(
            "CREATE INDEX IF NOT EXISTS {name} ON {schema}.search_embeddings
             USING hnsw ((embedding::{vs}.vector({d})) {vs}.vector_cosine_ops)
             WHERE model = {literal} AND dimensions = {d}"
        ))
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// すべてのワークスペースに [`ensure_vector_index`] を行う（サーバーの起動時。モデルを変えたときのため）。
pub async fn ensure_vector_indexes(migration: &PgPool, config: &Config) -> Result<()> {
    if embedding_models(config).is_empty() {
        return Ok(());
    }
    let mut conn = migration.acquire().await?;
    if vector_schema(&mut *conn).await?.is_none() {
        return Ok(());
    }
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM cms.workspaces ORDER BY created_at")
        .fetch_all(&mut *conn)
        .await?;
    for id in ids {
        ensure_vector_index(&mut conn, &WorkspaceDb::names_for(id), config).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_index_names_differ_by_model_and_dimensions() {
        let a = vector_index_name("text-embedding-3-small", 1536);
        assert_eq!(a, vector_index_name("text-embedding-3-small", 1536));
        assert_ne!(a, vector_index_name("text-embedding-3-small", 512));
        assert_ne!(a, vector_index_name("text-embedding-3-large", 1536));
        assert!(a.as_str().starts_with("search_embeddings_hnsw_"));
    }
}
