//! 記事と下書きのDBの操作（ワークスペースのロールに切り替えたトランザクションの中で呼ぶ）。
//!
//! - 下書きは `ct_<コンテンツタイプ>` の列に置く（Q201）。リビジョン番号は `entries.draft_revision`（Q137, Q149）。
//! - 下書きの保存は、リビジョン番号を照合してから行う。ずれていれば何も書かない（INV-EDIT-01）。
//! - 識別子は検査済みのマニフェストの名前から作り、[`Ident`] を通して埋め込む。値はパラメーターで渡す。

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{Map, Value};
use sqlx::{PgConnection, Postgres, query::Query};
use uuid::Uuid;

use super::fields::Column;
use crate::db::Ident;
use crate::schema::manifest::{ContentType, FieldType, Manifest};
use crate::schema::migrate::{column_name, table_name};

const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;

type Q<'q> = Query<'q, Postgres, sqlx::postgres::PgArguments>;

fn bind_column<'q>(q: Q<'q>, c: &'q Column) -> Q<'q> {
    match c {
        Column::Text(v) | Column::Json(v) => q.bind(v.as_deref()),
        Column::Uuid(v) => q.bind(*v),
        Column::Uuids(v) => q.bind(v.as_deref()),
    }
}

/// 下書き（記事の情報と、フィールドの値）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub id: Uuid,
    pub content_type: String,
    pub locale: String,
    pub translation_group_id: Uuid,
    pub revision: i64,
    pub published_version_id: Option<Uuid>,
    pub created_at: String,
    pub updated_at: String,
    pub fields: Map<String, Value>,
}

/// 一覧に出す情報。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub id: Uuid,
    pub content_type: String,
    pub locale: String,
    pub translation_group_id: Uuid,
    pub revision: i64,
    pub published: bool,
    pub updated_at: String,
    /// 1行の文字列とスラッグのフィールドだけ（本文などの大きな値は含めない）。
    pub fields: Map<String, Value>,
}

/// `ct_<名前>` の行を、フィールド名 → 値 にする（マニフェストにあるフィールドだけ）。
async fn field_values(
    conn: &mut PgConnection,
    ct: &ContentType,
    entry_id: Uuid,
    only_short: bool,
) -> Result<Map<String, Value>> {
    let table = table_name(&ct.name)?;
    let row: Option<String> = sqlx::query_scalar(crate::sql!(
        "SELECT to_jsonb(t)::text FROM {table} t WHERE t.entry_id = $1"
    ))
    .bind(entry_id)
    .fetch_optional(&mut *conn)
    .await?;
    let mut raw: Map<String, Value> = match row {
        Some(r) => serde_json::from_str(&r)?,
        None => Map::new(),
    };
    let mut out = Map::new();
    for f in &ct.fields {
        if only_short && !matches!(f.kind, FieldType::Text | FieldType::Slug) {
            continue;
        }
        let value = raw.remove(&format!("f_{}", f.name)).unwrap_or(Value::Null);
        out.insert(f.name.clone(), value);
    }
    Ok(out)
}

type EntryRow = (
    Uuid,
    String,
    String,
    Uuid,
    i64,
    Option<Uuid>,
    String,
    String,
);

/// 下書きを読む（削除した記事や、マニフェストにないコンテンツタイプの記事は None）。
pub async fn draft(
    conn: &mut PgConnection,
    manifest: &Manifest,
    entry_id: Uuid,
) -> Result<Option<Draft>> {
    let row: Option<EntryRow> = sqlx::query_as(crate::sql!(
        "SELECT id, content_type, locale, translation_group_id, draft_revision, published_version_id,
                to_char(created_at AT TIME ZONE 'UTC', {RFC3339}), to_char(updated_at AT TIME ZONE 'UTC', {RFC3339})
         FROM entries WHERE id = $1 AND deleted_at IS NULL"
    ))
    .bind(entry_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((
        id,
        content_type,
        locale,
        translation_group_id,
        revision,
        published_version_id,
        created_at,
        updated_at,
    )) = row
    else {
        return Ok(None);
    };
    let Some(ct) = manifest.content_type(&content_type) else {
        return Ok(None);
    };
    let fields = field_values(conn, ct, id, false).await?;
    Ok(Some(Draft {
        id,
        content_type,
        locale,
        translation_group_id,
        revision,
        published_version_id,
        created_at,
        updated_at,
        fields,
    }))
}

/// 記事の一覧（新しく更新した順）。
pub async fn list(conn: &mut PgConnection, manifest: &Manifest) -> Result<Vec<Summary>> {
    let rows: Vec<(Uuid, String, String, Uuid, i64, bool, String)> = sqlx::query_as(crate::sql!(
        "SELECT id, content_type, locale, translation_group_id, draft_revision, published_version_id IS NOT NULL,
                to_char(updated_at AT TIME ZONE 'UTC', {RFC3339})
         FROM entries WHERE deleted_at IS NULL ORDER BY updated_at DESC, id"
    ))
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, content_type, locale, translation_group_id, revision, published, updated_at) in rows {
        let Some(ct) = manifest.content_type(&content_type) else {
            continue;
        };
        let fields = field_values(conn, ct, id, true).await?;
        out.push(Summary {
            id,
            content_type,
            locale,
            translation_group_id,
            revision,
            published,
            updated_at,
            fields,
        });
    }
    Ok(out)
}

/// 同じコンテンツタイプ・ロケールで、そのスラッグを使っている別の記事があるか。
pub async fn slug_taken(
    conn: &mut PgConnection,
    ct: &ContentType,
    locale: &str,
    slug: &str,
    except: Uuid,
) -> Result<bool> {
    let Some(slug_field) = ct.fields.iter().find(|f| f.kind == FieldType::Slug) else {
        return Ok(false);
    };
    let table = table_name(&ct.name)?;
    let column = column_name(&slug_field.name)?;
    Ok(sqlx::query_scalar(crate::sql!(
        "SELECT EXISTS (SELECT 1 FROM {table} t JOIN entries e ON e.id = t.entry_id
                        WHERE e.deleted_at IS NULL AND e.locale = $1 AND t.{column} = $2 AND e.id <> $3)"
    ))
    .bind(locale)
    .bind(slug)
    .bind(except)
    .fetch_one(conn)
    .await?)
}

/// 参照する翻訳グループが、すべて指定のコンテンツタイプの記事として存在するか。
pub async fn groups_exist(
    conn: &mut PgConnection,
    content_type: &str,
    groups: &[Uuid],
) -> Result<bool> {
    if groups.is_empty() {
        return Ok(true);
    }
    let mut unique = groups.to_vec();
    unique.sort();
    unique.dedup();
    let found: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT translation_group_id) FROM entries
         WHERE translation_group_id = ANY($1) AND content_type = $2 AND deleted_at IS NULL",
    )
    .bind(&unique)
    .bind(content_type)
    .fetch_one(conn)
    .await?;
    Ok(found == unique.len() as i64)
}

async fn write_columns(
    conn: &mut PgConnection,
    table: &Ident,
    entry_id: Uuid,
    columns: &[(String, Column)],
) -> Result<()> {
    if columns.is_empty() {
        return Ok(());
    }
    let mut sets = Vec::with_capacity(columns.len());
    for (i, (name, value)) in columns.iter().enumerate() {
        let column = column_name(name)?;
        let ty = value.sql_type();
        sets.push(format!("{column} = ${}::{ty}", i + 2));
    }
    let sets = sets.join(", ");
    let sql = crate::sql!("UPDATE {table} SET {sets} WHERE entry_id = $1");
    let mut q = sqlx::query(sql).bind(entry_id);
    for (_, value) in columns {
        q = bind_column(q, value);
    }
    q.execute(conn).await?;
    Ok(())
}

/// あと `adding` 件の記事を作っても、数の上限に収まるか（物理削除を待つ記事も、ディスクを使うので数える）。
/// 上限で数えるのをやめるので、記事が多くても時間はかからない。同時に作ると少し超えることがある。
pub async fn room_for(conn: &mut PgConnection, limit: u32, adding: usize) -> Result<bool> {
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM (SELECT 1 FROM entries LIMIT $1) t")
        .bind(i64::from(limit))
        .fetch_one(conn)
        .await?;
    Ok(count.saturating_add(adding as i64) <= i64::from(limit))
}

/// 記事を作る（下書き）。
pub async fn create(
    conn: &mut PgConnection,
    ct: &ContentType,
    entry_id: Uuid,
    locale: &str,
    translation_group_id: Uuid,
    created_by: Uuid,
    columns: &[(String, Column)],
) -> Result<()> {
    sqlx::query(
        "INSERT INTO entries (id, content_type, locale, translation_group_id, created_by)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(entry_id)
    .bind(&ct.name)
    .bind(locale)
    .bind(translation_group_id)
    .bind(created_by)
    .execute(&mut *conn)
    .await?;
    let table = table_name(&ct.name)?;
    sqlx::query(crate::sql!("INSERT INTO {table} (entry_id) VALUES ($1)"))
        .bind(entry_id)
        .execute(&mut *conn)
        .await?;
    write_columns(conn, &table, entry_id, columns).await
}

/// 下書きの保存の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Saved {
    /// 保存した。新しいリビジョン番号。
    Ok(i64),
    /// リビジョン番号が最新でない（何も書いていない）。
    Conflict,
}

/// 下書きを保存する。`expected` が最新のリビジョン番号と一致するときだけ書く。
pub async fn save(
    conn: &mut PgConnection,
    ct: &ContentType,
    entry_id: Uuid,
    expected: i64,
    columns: &[(String, Column)],
) -> Result<Saved> {
    let revision: Option<i64> = sqlx::query_scalar(
        "UPDATE entries SET draft_revision = draft_revision + 1, updated_at = now()
         WHERE id = $1 AND draft_revision = $2 AND deleted_at IS NULL
         RETURNING draft_revision",
    )
    .bind(entry_id)
    .bind(expected)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(revision) = revision else {
        return Ok(Saved::Conflict);
    };
    let table = table_name(&ct.name)?;
    write_columns(conn, &table, entry_id, columns)
        .await
        .context("cannot write draft fields")?;
    Ok(Saved::Ok(revision))
}
