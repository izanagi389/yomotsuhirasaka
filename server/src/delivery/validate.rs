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

use super::names::{ANSWER_FIELD, ANSWER_STREAM_FIELD, VISITOR_ID_ARG};
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
    /// ルートのフィールドの名前（フラグメントの中のものも含む。`__typename` だけは含めない。`__schema` などは含める）。
    pub roots: Vec<String>,
    /// 回答のフィールド（`answer`・`answerStream`）を選んだ回数（別名・フラグメントの分も数える）。
    pub answer_fields: usize,
    /// 回答のフィールドの `visitorId` の引数（回数制限を、実行の前に数えるため）。
    pub visitor: VisitorArg,
}

/// 回答のフィールドの `visitorId` の引数の書き方。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum VisitorArg {
    #[default]
    Absent,
    /// 文に直接書いた値。
    Literal(async_graphql::Value),
    /// 変数（`default` は変数の既定値）。
    Variable {
        name: String,
        default: Option<async_graphql::Value>,
    },
}

/// `visitorId` の値が文字列でない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidVisitorId;

impl Shape {
    /// 回答のフィールドを含むか。
    pub fn answering(&self) -> bool {
        self.answer_fields > 0
    }

    /// 回答のフィールドを、ほかのフィールドと混ぜずに1回だけ選んでいるか（1回のリクエストでLLMを何度も呼ばせない）。
    pub fn answer_alone(&self) -> bool {
        self.answer_fields == 1 && self.roots.len() == 1
    }

    /// 回答のフィールドの `visitorId` の値（変数は、送られた値か、なければ既定値で解く。実行のときと同じ）。
    /// 指定がない・null なら None。
    pub fn visitor_id(&self, variables: &Value) -> Result<Option<String>, InvalidVisitorId> {
        let value = match &self.visitor {
            VisitorArg::Absent => None,
            VisitorArg::Literal(v) => Some(v.clone()),
            VisitorArg::Variable { name, default } => match variables.get(name) {
                Some(json) => Some(
                    async_graphql::Value::from_json(json.clone()).map_err(|_| InvalidVisitorId)?,
                ),
                None => default.clone(),
            },
        };
        match value {
            None | Some(async_graphql::Value::Null) => Ok(None),
            Some(async_graphql::Value::String(s)) => Ok(Some(s)),
            Some(_) => Err(InvalidVisitorId),
        }
    }
}

fn is_answer(name: &str) -> bool {
    name == ANSWER_FIELD || name == ANSWER_STREAM_FIELD
}

/// 文の形を読む。1つの操作だけを含む文でなければ None。フラグメントが深すぎて読み切れない文も None（数え漏れたフィールドの
/// スコープを確かめずに通さないため。読めない文は拒否する）。
pub fn shape(query: &str) -> Option<Shape> {
    let doc = async_graphql::parser::parse_query(query).ok()?;
    let op = match &doc.operations {
        DocumentOperations::Single(op) => op,
        DocumentOperations::Multiple(ops) if ops.len() == 1 => ops.values().next()?,
        DocumentOperations::Multiple(_) => return None,
    };
    let mut shape = Shape {
        subscription: op.node.ty == OperationType::Subscription,
        roots: Vec::new(),
        answer_fields: 0,
        visitor: VisitorArg::Absent,
    };
    let mut visited = 0;
    collect_roots(
        &doc,
        &op.node.selection_set.node,
        &mut shape,
        &mut visited,
        0,
    )?;
    if let VisitorArg::Variable { name, default } = &mut shape.visitor {
        *default = op
            .node
            .variable_definitions
            .iter()
            .find(|d| d.node.name.node.as_str() == name)
            .and_then(|d| d.node.default_value.as_ref())
            .map(|v| v.node.clone());
    }
    Some(shape)
}

/// フラグメントをたどる深さの上限。超えたら読み切れないものとする。
const MAX_FRAGMENT_DEPTH: usize = 32;
/// たどる選択の数の上限（同じフラグメントを何度も使って、たどる数を倍々に増やす文を止める）。超えたら読み切れないものとする。
const MAX_VISITED_SELECTIONS: usize = 10_000;

fn collect_roots(
    doc: &ExecutableDocument,
    set: &SelectionSet,
    out: &mut Shape,
    visited: &mut usize,
    depth: usize,
) -> Option<()> {
    // フラグメントの循環は検証で拒否されるが、ここでも深さで止める
    if depth > MAX_FRAGMENT_DEPTH {
        return None;
    }
    for item in &set.items {
        *visited += 1;
        if *visited > MAX_VISITED_SELECTIONS {
            return None;
        }
        match &item.node {
            Selection::Field(f) => {
                let name = f.node.name.node.as_str();
                if is_answer(name) {
                    out.answer_fields += 1;
                    out.visitor = match f.node.get_argument(VISITOR_ID_ARG) {
                        None => VisitorArg::Absent,
                        Some(v) => {
                            let mut variable = None;
                            let literal = v
                                .node
                                .clone()
                                .into_const_with(|name| {
                                    variable = Some(name.to_string());
                                    Ok::<_, std::convert::Infallible>(async_graphql::Value::Null)
                                })
                                .unwrap_or(async_graphql::Value::Null);
                            match variable {
                                Some(name) => VisitorArg::Variable {
                                    name,
                                    default: None,
                                },
                                None => VisitorArg::Literal(literal),
                            }
                        }
                    };
                }
                // `__typename` は何も読まない。`__schema`・`__type` はスキーマを読むので、ほかのフィールドと同じに数える
                if name != "__typename" && !out.roots.iter().any(|n| n == name) {
                    out.roots.push(name.to_owned());
                }
            }
            Selection::InlineFragment(f) => {
                collect_roots(doc, &f.node.selection_set.node, out, visited, depth + 1)?
            }
            Selection::FragmentSpread(s) => {
                // 定義のないフラグメントは、中身を数えられないので拒否する
                let f = doc.fragments.get(&s.node.fragment_name.node)?;
                collect_roots(doc, &f.node.selection_set.node, out, visited, depth + 1)?;
            }
        }
    }
    Some(())
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
    // 回答のフィールドは、1つの文に1回だけ、ほかのフィールドと混ぜずに選ぶ（別名で何度も選ぶと、1回のリクエストで
    // LLMを何度も呼べてしまう）
    // 実行のときに読めない文（フラグメントが深すぎる）は、登録もさせない
    let Some(shape) = shape(query) else {
        return Err(vec![
            "the query nests fragments too deeply or uses an undefined fragment".to_owned(),
        ]);
    };
    if shape.answering() && !shape.answer_alone() {
        return Err(vec![
            "select `answer` or `answerStream` once, without other fields".to_owned(),
        ]);
    }
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
        db: None,
        resolved: Default::default(),
        max_resolved: usize::MAX,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answer_fields_are_counted_through_aliases_and_fragments() {
        let one = shape(r#"query { answer(question: "q", locale: "ja") { text } }"#).unwrap();
        assert!(one.answering() && one.answer_alone());
        let aliased = shape(
            r#"query { a: answer(question: "q", locale: "ja") { text } b: answer(question: "q", locale: "ja") { text } }"#,
        )
        .unwrap();
        assert_eq!(aliased.answer_fields, 2);
        assert!(!aliased.answer_alone());
        let via_fragment = shape(
            r#"query { ...F answer(question: "q", locale: "ja") { text } } fragment F on Query { answer(question: "x", locale: "ja") { text } }"#,
        )
        .unwrap();
        assert_eq!(via_fragment.answer_fields, 2);
        let mixed =
            shape(r#"query { answer(question: "q", locale: "ja") { text } redirects { from } }"#)
                .unwrap();
        assert!(mixed.answering() && !mixed.answer_alone());
        assert!(!shape("query { redirects { from } }").unwrap().answering());
    }

    #[test]
    fn unreadable_documents_are_refused() {
        // 33段より深いフラグメント（奥のフィールドを数えられない）
        let mut q = String::from("query { ...F0 }");
        for i in 0..40 {
            q.push_str(&format!(" fragment F{i} on Query {{ ...F{} }}", i + 1));
        }
        q.push_str(" fragment F40 on Query { redirects { from } }");
        assert_eq!(shape(&q), None);
        // 定義のないフラグメント
        assert_eq!(shape("query { ...Missing }"), None);
        // 同じフラグメントを2回ずつ使って、たどる数を倍々に増やす文
        let mut q = String::from("query { ...G0 }");
        for i in 0..30 {
            q.push_str(&format!(
                " fragment G{i} on Query {{ ...G{n} ...G{n} }}",
                n = i + 1
            ));
        }
        q.push_str(" fragment G30 on Query { redirects { from } }");
        assert_eq!(shape(&q), None);
        // イントロスペクションは、ほかのフィールドと同じに数える（回答と混ぜられない）
        let meta =
            shape(r#"query { __schema { queryType { name } } answer(question: "q", locale: "ja") { text } }"#)
                .unwrap();
        assert!(meta.answering() && !meta.answer_alone());
        assert!(
            shape(r#"query { __typename answer(question: "q", locale: "ja") { text } }"#)
                .unwrap()
                .answer_alone()
        );
        // ふつうの深さなら読める
        let ok = shape("query { ...A } fragment A on Query { ...B } fragment B on Query { redirects { from } }").unwrap();
        assert_eq!(ok.roots, vec!["redirects".to_owned()]);
    }
}
