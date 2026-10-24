//! クエリの検証（登録のときと、テンプレートのデプロイのとき。13-security.md「GraphQL」、02-schema-and-delivery-api.md「互換性の保護」）。
//!
//! - 1つの問い合わせ（query）だけを含むこと。ミューテーションなどは受け付けない。
//! - スキーマに対する検証と、深さ・複雑さの上限。仮の変数で実行するが、データは読まない（[`RequestData::tx`] を None にする）。

use std::sync::Arc;

use async_graphql::dynamic::Schema;
use async_graphql::parser::types::{BaseType, DocumentOperations, OperationType, Type};
use serde_json::{Map, Value};
use tokio::sync::Mutex;
use uuid::Uuid;

use super::queries::MAX_QUERY_BYTES;
use super::schema::RequestData;

/// 検証のための仮の変数の値（型に合う最小の値）。
fn placeholder(ty: &Type) -> Value {
    if ty.nullable {
        return Value::Null;
    }
    match &ty.base {
        BaseType::List(_) => Value::Array(Vec::new()),
        BaseType::Named(name) => match name.as_str() {
            "Int" => Value::from(0),
            "Float" => Value::from(0.0),
            "Boolean" => Value::Bool(false),
            _ => Value::String(String::new()),
        },
    }
}

/// クエリを検証する。問題があれば、その説明の一覧を返す。
pub async fn check(schema: &Schema, query: &str) -> Result<(), Vec<String>> {
    if query.len() > MAX_QUERY_BYTES {
        return Err(vec!["the query is too long".to_owned()]);
    }
    let doc = async_graphql::parser::parse_query(query).map_err(|e| vec![e.to_string()])?;
    let op = match &doc.operations {
        DocumentOperations::Single(op) => op,
        DocumentOperations::Multiple(ops) if ops.len() == 1 => {
            ops.values().next().expect("one operation")
        }
        DocumentOperations::Multiple(_) => {
            return Err(vec![
                "a registered query must contain exactly one operation".to_owned(),
            ]);
        }
    };
    if op.node.ty != OperationType::Query {
        return Err(vec!["only queries can be registered".to_owned()]);
    }
    let variables: Map<String, Value> = op
        .node
        .variable_definitions
        .iter()
        .map(|v| {
            (
                v.node.name.node.to_string(),
                placeholder(&v.node.var_type.node),
            )
        })
        .collect();
    let data = Arc::new(RequestData {
        tx: Mutex::new(None),
        workspace_id: Uuid::nil(),
        media_base: String::new(),
        preview: None,
    });
    let request = async_graphql::Request::new(query.to_owned())
        .variables(async_graphql::Variables::from_json(Value::Object(
            variables,
        )))
        .data(data);
    let response = schema.execute(request).await;
    if response.errors.is_empty() {
        Ok(())
    } else {
        Err(response.errors.into_iter().map(|e| e.message).collect())
    }
}
