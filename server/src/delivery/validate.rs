//! クエリの検証（登録のときと、テンプレートのデプロイのとき。13-security.md「GraphQL」、02-schema-and-delivery-api.md「互換性の保護」）。
//!
//! - 1つの問い合わせ（query）か、回答のストリーミングの購読（subscription。M4）だけを含むこと。ミューテーションなどは受け付けない。
//! - スキーマに対する検証と、深さ・複雑さの上限。仮の変数で実行するが、データは読まない（[`RequestData::tx`] を None にする）。

use std::sync::Arc;

use async_graphql::dynamic::Schema;
use async_graphql::futures_util::StreamExt as _;
use async_graphql::parser::types::{
    BaseType, DocumentOperations, ExecutableDocument, OperationType, Selection, SelectionSet, Type,
};
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

/// 実行する文の形（http::delivery が、スコープとメソッドを確かめるのに使う）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape {
    pub subscription: bool,
    /// ルートのフィールドの名前（フラグメントの中のものも含む。`__typename` などは含めない）。
    pub roots: Vec<String>,
}

/// 文の形を読む。1つの操作だけを含む文でなければ None。
pub fn shape(query: &str) -> Option<Shape> {
    let doc = async_graphql::parser::parse_query(query).ok()?;
    let op = match &doc.operations {
        DocumentOperations::Single(op) => op,
        DocumentOperations::Multiple(ops) if ops.len() == 1 => ops.values().next()?,
        DocumentOperations::Multiple(_) => return None,
    };
    let mut roots = Vec::new();
    collect_roots(&doc, &op.node.selection_set.node, &mut roots, 0);
    Some(Shape {
        subscription: op.node.ty == OperationType::Subscription,
        roots,
    })
}

fn collect_roots(
    doc: &ExecutableDocument,
    set: &SelectionSet,
    out: &mut Vec<String>,
    depth: usize,
) {
    // フラグメントの循環は検証で拒否されるが、ここでも深さで止める
    if depth > 32 {
        return;
    }
    for item in &set.items {
        match &item.node {
            Selection::Field(f) => {
                let name = f.node.name.node.as_str();
                if !name.starts_with("__") && !out.iter().any(|n| n == name) {
                    out.push(name.to_owned());
                }
            }
            Selection::InlineFragment(f) => {
                collect_roots(doc, &f.node.selection_set.node, out, depth + 1)
            }
            Selection::FragmentSpread(s) => {
                if let Some(f) = doc.fragments.get(&s.node.fragment_name.node) {
                    collect_roots(doc, &f.node.selection_set.node, out, depth + 1);
                }
            }
        }
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
    let subscription = match op.node.ty {
        OperationType::Query => false,
        OperationType::Subscription => true,
        OperationType::Mutation => {
            return Err(vec![
                "only queries and the answer subscription can be registered".to_owned(),
            ]);
        }
    };
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
        app: None,
    });
    let request = async_graphql::Request::new(query.to_owned())
        .variables(async_graphql::Variables::from_json(Value::Object(
            variables,
        )))
        .data(data);
    let response = if subscription {
        // 購読は、検証の誤りがあれば最初の応答で返る（検証だけのときは、リゾルバーは何も流さない）
        match schema.execute_stream(request).next().await {
            Some(r) => r,
            None => return Ok(()),
        }
    } else {
        schema.execute(request).await
    };
    if response.errors.is_empty() {
        Ok(())
    } else {
        Err(response.errors.into_iter().map(|e| e.message).collect())
    }
}
