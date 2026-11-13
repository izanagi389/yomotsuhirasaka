//! ワークスペースへのテンプレートの適用（移行。Q53）。
//!
//! - コンテンツタイプごとにテーブル `ct_<名前>` を作り、フィールドを1列ずつ `f_<名前>` で持つ（下書きの値）。
//!   公開した版はJSONのスナップショット（`versions.fields`）で持つ（docs/history の Q201）。
//! - 識別子は、検査を通ったマニフェストの名前から作り、必ず [`Ident`] を通して埋め込む。値はパラメーターで渡す。
//! - 既存のデータが新しいスキーマを満たさない（必須になったフィールドが空の公開中の記事がある、など）ときは失敗にする。
//!   呼び出し側はトランザクションを巻き戻し、そのワークスペースを「移行待ち」にする。

use anyhow::Result;
use sqlx::PgConnection;

use super::diff::shape_changed;
use super::manifest::{ContentType, Field, FieldType, Manifest, types_by_name};
use crate::db::Ident;
use crate::workspace_db::WorkspaceDb;

/// 移行が失敗した理由。`code()` を状態として保存する（記事の内容は含めない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// 必須のフィールドが空の、公開中の記事がある。
    RequiredValueMissing { content_type: String, field: String },
    /// 外すロケールの記事がある。
    LocaleInUse(String),
    /// 外すノードやマークを使っている本文がある。
    BodyNodeInUse(String),
}

impl Failure {
    pub fn code(&self) -> String {
        match self {
            Self::RequiredValueMissing {
                content_type,
                field,
            } => format!("required_value_missing:{content_type}.{field}"),
            Self::LocaleInUse(l) => format!("locale_in_use:{l}"),
            Self::BodyNodeInUse(n) => format!("body_node_in_use:{n}"),
        }
    }
}

pub fn table_name(content_type: &str) -> Result<Ident> {
    Ok(Ident::new(&format!("ct_{content_type}"))?)
}

pub fn column_name(field: &str) -> Result<Ident> {
    Ok(Ident::new(&format!("f_{field}"))?)
}

/// フィールドの型に対応する列の型（固定の文字列だけ）。
fn column_type(f: &Field) -> &'static str {
    match (f.kind, f.multiple) {
        (FieldType::Text | FieldType::Slug, _) => "text",
        (FieldType::Richtext | FieldType::Image, _) => "jsonb",
        (FieldType::Reference, false) => "uuid",
        (FieldType::Reference, true) => "uuid[]",
    }
}

async fn exec(conn: &mut PgConnection, sql: sqlx::AssertSqlSafe<String>) -> Result<()> {
    sqlx::query(sql).execute(conn).await?;
    Ok(())
}

async fn create_table(conn: &mut PgConnection, db: &WorkspaceDb, ct: &ContentType) -> Result<()> {
    let schema = &db.schema;
    let table = table_name(&ct.name)?;
    exec(
        conn,
        crate::sql!(
            "CREATE TABLE {schema}.{table} (entry_id uuid PRIMARY KEY REFERENCES {schema}.entries (id) ON DELETE CASCADE)"
        ),
    )
    .await?;
    for f in &ct.fields {
        add_column(conn, db, &table, f).await?;
    }
    Ok(())
}

async fn add_column(
    conn: &mut PgConnection,
    db: &WorkspaceDb,
    table: &Ident,
    f: &Field,
) -> Result<()> {
    let schema = &db.schema;
    let column = column_name(&f.name)?;
    let ty = column_type(f);
    exec(
        conn,
        crate::sql!("ALTER TABLE {schema}.{table} ADD COLUMN {column} {ty}"),
    )
    .await
}

async fn drop_column(
    conn: &mut PgConnection,
    db: &WorkspaceDb,
    table: &Ident,
    field: &str,
) -> Result<()> {
    let schema = &db.schema;
    let column = column_name(field)?;
    exec(
        conn,
        crate::sql!("ALTER TABLE {schema}.{table} DROP COLUMN {column}"),
    )
    .await
}

/// 公開中の記事のうち、そのフィールドが空のものがあるか。
async fn published_value_missing(
    conn: &mut PgConnection,
    db: &WorkspaceDb,
    content_type: &str,
    field: &str,
) -> Result<bool> {
    let schema = &db.schema;
    Ok(sqlx::query_scalar(crate::sql!(
        "SELECT EXISTS (
             SELECT 1 FROM {schema}.entries e JOIN {schema}.versions v ON v.id = e.published_version_id
             WHERE e.deleted_at IS NULL AND e.content_type = $1 AND v.fields IS NOT NULL
               AND (v.fields -> $2 IS NULL OR v.fields -> $2 IN ('null'::jsonb, '\"\"'::jsonb, '[]'::jsonb)))"
    ))
    .bind(content_type)
    .bind(field)
    .fetch_one(conn)
    .await?)
}

/// 本文（公開中の版と下書き）で、そのノード・マークを使っているものがあるか。
async fn body_node_in_use(
    conn: &mut PgConnection,
    db: &WorkspaceDb,
    old: &Manifest,
    node: &str,
) -> Result<bool> {
    let schema = &db.schema;
    const PATH: &str = "'$.** ? (@.type == $n)'";
    let used: bool = sqlx::query_scalar(crate::sql!(
        "SELECT EXISTS (
             SELECT 1 FROM {schema}.entries e JOIN {schema}.versions v ON v.id = e.published_version_id
             WHERE e.deleted_at IS NULL AND jsonb_path_exists(v.fields, {PATH}, jsonb_build_object('n', $1::text)))"
    ))
    .bind(node)
    .fetch_one(&mut *conn)
    .await?;
    if used {
        return Ok(true);
    }
    for ct in &old.content_types {
        let table = table_name(&ct.name)?;
        for f in ct.fields.iter().filter(|f| f.kind == FieldType::Richtext) {
            let column = column_name(&f.name)?;
            let used: bool = sqlx::query_scalar(crate::sql!(
                "SELECT EXISTS (
                     SELECT 1 FROM {schema}.{table} t JOIN {schema}.entries e ON e.id = t.entry_id
                     WHERE e.deleted_at IS NULL AND jsonb_path_exists(t.{column}, {PATH}, jsonb_build_object('n', $1::text)))"
            ))
            .bind(node)
            .fetch_one(&mut *conn)
            .await?;
            if used {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// `old`（適用済み。まだなければ None）から `new` へ、ワークスペースのスキーマを移行する。
/// 呼び出し側のトランザクションの中で呼び、`Err(Failure)` なら巻き戻すこと。
pub async fn apply(
    conn: &mut PgConnection,
    db: &WorkspaceDb,
    old: Option<&Manifest>,
    new: &Manifest,
) -> Result<Result<(), Failure>> {
    let schema = &db.schema;
    let empty = Manifest {
        locales: Vec::new(),
        default_locale: String::new(),
        body: super::manifest::Body {
            nodes: Vec::new(),
            marks: Vec::new(),
            embed_providers: Vec::new(),
            custom_nodes: Vec::new(),
        },
        content_types: Vec::new(),
    };
    let old_m = old.unwrap_or(&empty);
    let old_types = types_by_name(old_m);
    let new_types = types_by_name(new);

    // データの検査（DDLの前に）
    for l in old_m.locales.iter().filter(|l| !new.locales.contains(l)) {
        let used: bool = sqlx::query_scalar(crate::sql!(
            "SELECT EXISTS (SELECT 1 FROM {schema}.entries WHERE deleted_at IS NULL AND locale = $1)"
        ))
        .bind(l)
        .fetch_one(&mut *conn)
        .await?;
        if used {
            return Ok(Err(Failure::LocaleInUse(l.clone())));
        }
    }
    let (new_all, old_all) = (new.body.all_nodes(), old_m.body.all_nodes());
    let new_nodes: Vec<&str> = new_all
        .iter()
        .map(|n| n.name.as_str())
        .chain(new.body.marks.iter().map(|m| m.name.as_str()))
        .collect();
    for name in old_all
        .iter()
        .map(|n| n.name.as_str())
        .chain(old_m.body.marks.iter().map(|m| m.name.as_str()))
        .filter(|n| !new_nodes.contains(n))
    {
        if body_node_in_use(conn, db, old_m, name).await? {
            return Ok(Err(Failure::BodyNodeInUse(name.to_owned())));
        }
    }
    for (name, nct) in &new_types {
        let oct = old_types.get(name);
        for nf in nct.fields.iter().filter(|f| f.required) {
            let was_required = oct
                .and_then(|o| o.field(&nf.name))
                .is_some_and(|of| of.required && !shape_changed(of, nf));
            if !was_required && published_value_missing(conn, db, name, &nf.name).await? {
                return Ok(Err(Failure::RequiredValueMissing {
                    content_type: (*name).to_owned(),
                    field: nf.name.clone(),
                }));
            }
        }
    }

    // DDL
    for name in old_types.keys().filter(|n| !new_types.contains_key(*n)) {
        let table = table_name(name)?;
        exec(conn, crate::sql!("DROP TABLE {schema}.{table}")).await?;
    }
    for (name, nct) in &new_types {
        let Some(oct) = old_types.get(name) else {
            create_table(conn, db, nct).await?;
            continue;
        };
        let table = table_name(name)?;
        for of in &oct.fields {
            match nct.field(&of.name) {
                None => drop_column(conn, db, &table, &of.name).await?,
                Some(nf) if shape_changed(of, nf) => {
                    drop_column(conn, db, &table, &of.name).await?;
                    add_column(conn, db, &table, nf).await?;
                }
                Some(_) => {}
            }
        }
        for nf in nct.fields.iter().filter(|f| oct.field(&f.name).is_none()) {
            add_column(conn, db, &table, nf).await?;
        }
    }
    Ok(Ok(()))
}
