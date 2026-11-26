//! 受け付ける値を狭める変更の移行（2026-11-25、02-schema-and-delivery-api.md「フィールドの型」）。
//!
//! - 選択肢を外す・範囲を狭める・グループの件数の上限を下げる・グループの中のフィールドを必須にする、などの変更では、
//!   下書き・公開中の版・予約版の値を新しい定義で確かめ、合わないものが1つでもあれば移行を止める（記事のIDを示す）。
//!   公開中の版と予約版では、グループの中の必須のフィールドがそろっていることも確かめる（下書きは書きかけでよい）。
//! - グループの中で外したフィールドと型を変えたフィールドの値は、直下のフィールドの列と同じく消す（下書きから取り除く）。
//!   公開中の版と予約版には残るが、配信では新しい定義にないものとして読まない。

use std::collections::BTreeSet;

use anyhow::Result;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use super::diff::shape_changed;
use super::manifest::{Field, FieldType, GROUP_ITEM_ID_KEY};
use super::migrate::{column_name, table_name};
use crate::content::{fields, groups};
use crate::workspace_db::WorkspaceDb;

/// 移行の失敗の理由に載せる記事の数の上限。
const MAX_LISTED_ENTRIES: usize = 10;

/// 古い定義の値のうち、新しい定義で残すもの。グループの中で、外れたフィールドと型が変わったフィールドの値を除く。
fn kept_value(old: &Field, new: &Field, value: &Value) -> Value {
    if new.kind != FieldType::Group {
        return value.clone();
    }
    let keep = |key: &str| {
        key == GROUP_ITEM_ID_KEY
            || matches!((old.inner(key), new.inner(key)), (Some(o), Some(n)) if !shape_changed(o, n))
    };
    match value {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| match item {
                    Value::Object(obj) => Value::Object(
                        obj.iter()
                            .filter(|(k, _)| keep(k))
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect(),
                    ),
                    other => other.clone(),
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// 新しい定義に合うか。`complete`（公開中の版と予約版）なら、グループの中の必須のフィールドもそろっていること。
fn fits(new: &Field, value: &Value, complete: bool) -> bool {
    if value.is_null() {
        return true;
    }
    if new.kind != FieldType::Group {
        return fields::value_of(new, value).is_ok();
    }
    let mut problems = Vec::new();
    let Some(normalized) = groups::normalize("", new, value, &mut BTreeSet::new(), &mut problems)
    else {
        return false;
    };
    !complete
        || groups::missing_required(new, Some(&normalized))
            .next()
            .is_none()
}

/// 新しい定義に合わない値を持つ記事（下書き・公開中の版・予約版。最大 [`MAX_LISTED_ENTRIES`] 件）。
pub async fn violations(
    conn: &mut PgConnection,
    db: &WorkspaceDb,
    content_type: &str,
    old: &Field,
    new: &Field,
) -> Result<Vec<Uuid>> {
    let schema = &db.schema;
    let table = table_name(content_type)?;
    let column = column_name(&old.name)?;
    let drafts: Vec<(Uuid, String)> = sqlx::query_as(crate::sql!(
        "SELECT t.entry_id, to_jsonb(t.{column})::text FROM {schema}.{table} t
         JOIN {schema}.entries e ON e.id = t.entry_id
         WHERE e.deleted_at IS NULL AND t.{column} IS NOT NULL"
    ))
    .fetch_all(&mut *conn)
    .await?;
    let published: Vec<(Uuid, String)> = sqlx::query_as(crate::sql!(
        "SELECT e.id, (v.fields -> $2)::text FROM {schema}.entries e
         JOIN {schema}.versions v ON v.id = e.published_version_id
         WHERE e.deleted_at IS NULL AND e.content_type = $1 AND v.fields -> $2 IS NOT NULL"
    ))
    .bind(content_type)
    .bind(&old.name)
    .fetch_all(&mut *conn)
    .await?;
    let scheduled: Vec<(Uuid, String)> = sqlx::query_as(crate::sql!(
        "SELECT e.id, (s.prepared -> 'fields' -> $2)::text FROM {schema}.schedules s
         JOIN {schema}.entries e ON e.id = s.entry_id
         WHERE s.state = 'pending' AND e.deleted_at IS NULL AND e.content_type = $1
           AND s.prepared -> 'fields' -> $2 IS NOT NULL"
    ))
    .bind(content_type)
    .bind(&old.name)
    .fetch_all(&mut *conn)
    .await?;

    let mut found = BTreeSet::new();
    let rows = drafts
        .iter()
        .map(|r| (r, false))
        .chain(published.iter().chain(&scheduled).map(|r| (r, true)));
    for ((entry_id, raw), complete) in rows {
        let value: Value = serde_json::from_str(raw)?;
        if !fits(new, &kept_value(old, new, &value), complete) {
            found.insert(*entry_id);
        }
    }
    Ok(found.into_iter().take(MAX_LISTED_ENTRIES).collect())
}

/// 下書きのグループの値から、外れたフィールドと型が変わったフィールドの値を取り除く。
pub async fn strip_group_drafts(
    conn: &mut PgConnection,
    db: &WorkspaceDb,
    content_type: &str,
    old: &Field,
    new: &Field,
) -> Result<()> {
    if new.kind != FieldType::Group {
        return Ok(());
    }
    let schema = &db.schema;
    let table = table_name(content_type)?;
    let column = column_name(&old.name)?;
    let drafts: Vec<(Uuid, String)> = sqlx::query_as(crate::sql!(
        "SELECT entry_id, {column}::text FROM {schema}.{table} WHERE {column} IS NOT NULL"
    ))
    .fetch_all(&mut *conn)
    .await?;
    let mut ids = Vec::new();
    let mut values = Vec::new();
    for (entry_id, raw) in drafts {
        let value: Value = serde_json::from_str(&raw)?;
        let kept = kept_value(old, new, &value);
        if kept != value {
            ids.push(entry_id);
            values.push(kept.to_string());
        }
    }
    if ids.is_empty() {
        return Ok(());
    }
    // 変わった下書きを1回でまとめて書く
    sqlx::query(crate::sql!(
        "UPDATE {schema}.{table} t SET {column} = u.value::jsonb
         FROM unnest($1::uuid[], $2::text[]) AS u(entry_id, value)
         WHERE t.entry_id = u.entry_id"
    ))
    .bind(&ids)
    .bind(&values)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn field(v: Value) -> Field {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn group_values_are_checked_against_the_new_definition() {
        let old = field(json!({"name": "faq", "type": "group", "fields": [
            {"name": "q", "type": "text"},
            {"name": "n", "type": "number"},
            {"name": "gone", "type": "text"}
        ]}));
        let new = field(
            json!({"name": "faq", "type": "group", "maxItems": 2, "fields": [
                {"name": "q", "type": "text", "required": true},
                {"name": "n", "type": "date"}
            ]}),
        );
        let value = json!([{"id": "i-1", "q": "a", "n": 3, "gone": "x"}]);
        let kept = kept_value(&old, &new, &value);
        assert_eq!(kept, json!([{"id": "i-1", "q": "a"}]));
        assert!(fits(&new, &kept, true));

        let missing = kept_value(&old, &new, &json!([{"id": "i-1", "n": 3}]));
        assert!(fits(&new, &missing, false), "下書きは書きかけでよい");
        assert!(
            !fits(&new, &missing, true),
            "公開中の版は必須がそろっていること"
        );
        assert!(
            !fits(&new, &json!([{"q": "a"}, {"q": "b"}, {"q": "c"}]), false),
            "件数の上限"
        );
    }

    #[test]
    fn scalar_values_are_checked_against_the_new_definition() {
        let new = field(json!({"name": "s", "type": "select", "options": [{"value": "a"}]}));
        assert!(fits(&new, &json!("a"), false));
        assert!(!fits(&new, &json!("b"), false));
        let new = field(json!({"name": "n", "type": "number", "max": 5}));
        assert!(fits(&new, &json!(5), true) && !fits(&new, &json!(5.5), true));
        assert!(fits(&new, &Value::Null, true));
    }
}
