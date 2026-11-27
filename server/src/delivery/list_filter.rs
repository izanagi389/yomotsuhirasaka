//! 配信APIの一覧の絞り込みと並べ替え（M5、2026-11-26、02-schema-and-delivery-api.md「フィールドの型」）。
//!
//! - 絞り込めるのは、コンテンツタイプの直下の選択肢・真偽値・参照（等しい・含む）と、数値・日付・日時（範囲）のフィールド。
//!   グループの中のフィールドは使えない。条件はすべてを満たすもの（AND）で、[`MAX_CONDITIONS`] 個まで（INV-FLD-04）。
//! - 並べ替えのキーは1つだけ（数値・日付・日時のフィールドか、公開日時）。値のない記事は、向きにかかわらず最後に並べる。
//! - 条件は公開中の版の値に当てる（INV-FLD-03。SQL は `data::list` の `versions v` の行の `list_values` に当てる。本文を読まずに済むよう、公開した版の短い値をDBが写した列）。参照のフィールドは、
//!   参照先の翻訳グループに、一覧と同じロケールの公開中の記事があるときだけ一致する（配信で参照を解決するときと同じ照合。原則3）。
//! - 条件の数とキーの数は、実行のたびに確かめる。文に書き込んだ条件は、登録の検査（仮の実行）でも同じ検査にかかる。
//! - SQL には定数とプレースホルダーだけを書き、フィールドの名前も値もパラメーターで渡す（13-security.md「SQL」）。

use std::collections::BTreeMap;
use std::fmt::Write as _;

use async_graphql::dynamic::{Enum, EnumItem, InputObject, InputValue, ObjectAccessor, TypeRef};
use uuid::Uuid;

use crate::content::datetime;
use crate::schema::manifest::{ContentType, Field, FieldType};

/// 1回の一覧で使える絞り込みの条件の数の上限（INV-FLD-04）。
pub const MAX_CONDITIONS: usize = 5;

pub const WHERE_ARG: &str = "where";
pub const ORDER_BY_ARG: &str = "orderBy";
/// 並べ替えのキーにできる公開日時（テンプレートのフィールド名は英小文字・数字・`_` なので、ぶつからない）。
const PUBLISHED_AT_KEY: &str = "publishedAt";

pub const SORT_DIRECTION_TYPE: &str = "YomiSortDirection";

/// 絞り込みの条件の種類（GraphQL の入力の型ごと）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilterKind {
    Select,
    Selects,
    Boolean,
    Reference,
    References,
    Int,
    Float,
    Date,
    DateTime,
}

impl FilterKind {
    const ALL: [Self; 9] = [
        Self::Select,
        Self::Selects,
        Self::Boolean,
        Self::Reference,
        Self::References,
        Self::Int,
        Self::Float,
        Self::Date,
        Self::DateTime,
    ];

    fn of(f: &Field) -> Option<Self> {
        Some(match f.kind {
            FieldType::Select if f.multiple => Self::Selects,
            FieldType::Select => Self::Select,
            FieldType::Boolean => Self::Boolean,
            FieldType::Reference if f.multiple => Self::References,
            FieldType::Reference => Self::Reference,
            FieldType::Number if f.integer => Self::Int,
            FieldType::Number => Self::Float,
            FieldType::Date => Self::Date,
            FieldType::Datetime => Self::DateTime,
            _ => return None,
        })
    }

    fn type_name(self) -> &'static str {
        match self {
            Self::Select => "YomiSelectFilter",
            Self::Selects => "YomiMultiSelectFilter",
            Self::Boolean => "YomiBooleanFilter",
            Self::Reference => "YomiReferenceFilter",
            Self::References => "YomiMultiReferenceFilter",
            Self::Int => "YomiIntRange",
            Self::Float => "YomiFloatRange",
            Self::Date => "YomiDateRange",
            Self::DateTime => "YomiDateTimeRange",
        }
    }

    /// （演算子, GraphQL の型）の一覧。
    fn operators(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Select => &[(EQ, TypeRef::STRING)],
            Self::Selects => &[(CONTAINS, TypeRef::STRING)],
            Self::Boolean => &[(EQ, TypeRef::BOOLEAN)],
            Self::Reference => &[(EQ, TypeRef::ID)],
            Self::References => &[(CONTAINS, TypeRef::ID)],
            Self::Int => &[(GTE, TypeRef::INT), (LTE, TypeRef::INT)],
            Self::Float => &[(GTE, TypeRef::FLOAT), (LTE, TypeRef::FLOAT)],
            Self::Date | Self::DateTime => &[(GTE, TypeRef::STRING), (LTE, TypeRef::STRING)],
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Select | Self::Boolean => "Entries whose value is `eq`.",
            Self::Selects => "Entries whose values include `contains`.",
            Self::Reference => {
                "Entries that reference the translation group `eq`, while it has a published entry in the same locale."
            }
            Self::References => {
                "Entries whose references include the translation group `contains`, while it has a published entry in the same locale."
            }
            Self::Int | Self::Float => {
                "Entries whose value is within the range (both ends included)."
            }
            Self::Date => {
                "Entries whose date (YYYY-MM-DD) is within the range (both ends included)."
            }
            Self::DateTime => {
                "Entries whose date and time is within the range (both ends included; RFC 3339 with an offset)."
            }
        }
    }
}

const EQ: &str = "eq";
const CONTAINS: &str = "contains";
const GTE: &str = "gte";
const LTE: &str = "lte";

/// 並べ替えのキーの値の比べ方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortKind {
    /// 数値として比べる。
    Number,
    /// そろえた書式の文字列（日付・UTC の日時）として、バイト順で比べる。
    Text,
}

impl SortKind {
    fn of(f: &Field) -> Option<Self> {
        match f.kind {
            FieldType::Number => Some(Self::Number),
            FieldType::Date | FieldType::Datetime => Some(Self::Text),
            _ => None,
        }
    }
}

/// コンテンツタイプの一覧で使える絞り込みと並べ替え。
#[derive(Debug, Clone, Default)]
pub struct ListSpec {
    filters: BTreeMap<String, (FilterKind, Option<String>)>,
    sorts: BTreeMap<String, SortKind>,
}

impl ListSpec {
    pub fn of(ct: &ContentType) -> Self {
        let mut spec = Self::default();
        for f in &ct.fields {
            if let Some(kind) = FilterKind::of(f) {
                spec.filters
                    .insert(f.name.clone(), (kind, f.reference_to.clone()));
            }
            if let Some(kind) = SortKind::of(f) {
                spec.sorts.insert(f.name.clone(), kind);
            }
        }
        spec
    }

    /// 絞り込めるフィールドがあるか（ないコンテンツタイプには `where` の引数を置かない）。
    pub fn filterable(&self) -> bool {
        !self.filters.is_empty()
    }

    /// `<型>Where` の入力の型。
    pub fn where_type(&self, name: &str) -> InputObject {
        self.filters.iter().fold(
            InputObject::new(name)
                .description("Conditions on the published version. All must hold; at most 5."),
            |o, (field, (kind, _))| {
                o.field(InputValue::new(field, TypeRef::named(kind.type_name())))
            },
        )
    }

    /// `<型>OrderBy` の入力の型。
    pub fn order_by_type(&self, name: &str) -> InputObject {
        self.sorts.keys().fold(
            InputObject::new(name)
                .description("One sort key. Entries without a value come last.")
                .field(InputValue::new(
                    PUBLISHED_AT_KEY,
                    TypeRef::named(SORT_DIRECTION_TYPE),
                )),
            |o, field| o.field(InputValue::new(field, TypeRef::named(SORT_DIRECTION_TYPE))),
        )
    }

    /// 引数 `where`・`orderBy` を読む。条件の数・キーの数・値の書式が合わなければ、理由を返す。
    pub fn read(
        &self,
        filter: Option<ObjectAccessor<'_>>,
        order: Option<ObjectAccessor<'_>>,
    ) -> Result<ListQuery, String> {
        let mut query = ListQuery::default();
        if let Some(filter) = filter {
            for (name, value) in filter.iter() {
                if value.is_null() {
                    continue;
                }
                let Some((kind, target)) = self.filters.get(name.as_str()) else {
                    return Err(format!("`{name}` cannot be used in `where`"));
                };
                let ops = value.object().map_err(|e| e.message)?;
                if let Some(c) = condition(name.as_str(), *kind, target.as_deref(), &ops)? {
                    query.conditions.push(c);
                }
            }
        }
        if query.conditions.len() > MAX_CONDITIONS {
            return Err(format!("`where` takes at most {MAX_CONDITIONS} conditions"));
        }
        if let Some(order) = order {
            let mut keys = order.iter().filter(|(_, v)| !v.is_null());
            if let Some((name, dir)) = keys.next() {
                if keys.next().is_some() {
                    return Err("`orderBy` takes exactly one key".to_owned());
                }
                let key = if name.as_str() == PUBLISHED_AT_KEY {
                    SortKey::PublishedAt
                } else {
                    match self.sorts.get(name.as_str()) {
                        Some(kind) => SortKey::Field {
                            key: name.to_string(),
                            kind: *kind,
                        },
                        None => return Err(format!("`{name}` cannot be used in `orderBy`")),
                    }
                };
                let descending = dir.enum_name().map_err(|e| e.message)? == DESC;
                query.sort = Some(Sort { key, descending });
            }
        }
        Ok(query)
    }
}

const ASC: &str = "ASC";
const DESC: &str = "DESC";

/// CMSが決める入力の型（絞り込みの条件の型と、並べ替えの向き）。スキーマに1回だけ登録する。
pub fn shared_types() -> (Vec<InputObject>, Enum) {
    let inputs = FilterKind::ALL
        .into_iter()
        .map(|kind| {
            kind.operators().iter().fold(
                InputObject::new(kind.type_name()).description(kind.description()),
                |o, (op, ty)| o.field(InputValue::new(*op, TypeRef::named(*ty))),
            )
        })
        .collect();
    let direction = Enum::new(SORT_DIRECTION_TYPE)
        .item(EnumItem::new(ASC))
        .item(EnumItem::new(DESC));
    (inputs, direction)
}

/// SQL に渡す値。
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    Text(String),
    Bool(bool),
    Int(i64),
    Float(f64),
    Uuid(Uuid),
}

/// 絞り込みの条件の1つ。
#[derive(Debug, Clone, PartialEq)]
enum Condition {
    /// 値が等しい（1つを選ぶ選択肢・真偽値）。
    Equals { key: String, value: Param },
    /// 配列の値が文字列を含む（複数を選べる選択肢）。
    Contains { key: String, value: String },
    /// 範囲（数値は数として、日付・日時はそろえた書式の文字列として比べる）。
    Range {
        key: String,
        kind: SortKind,
        gte: Option<Param>,
        lte: Option<Param>,
    },
    /// 参照先の翻訳グループ（公開中の記事があるものだけ一致する）。
    References {
        key: String,
        group: Uuid,
        multiple: bool,
        content_type: String,
    },
}

/// 並べ替えのキー。
#[derive(Debug, Clone, PartialEq)]
enum SortKey {
    Field { key: String, kind: SortKind },
    PublishedAt,
}

#[derive(Debug, Clone, PartialEq)]
struct Sort {
    key: SortKey,
    descending: bool,
}

/// 読んだ絞り込みと並べ替え。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListQuery {
    conditions: Vec<Condition>,
    sort: Option<Sort>,
}

/// 1つのフィールドの条件を読む。演算子がすべて null なら条件なし（None）。
fn condition(
    key: &str,
    kind: FilterKind,
    target: Option<&str>,
    ops: &ObjectAccessor<'_>,
) -> Result<Option<Condition>, String> {
    let op = |name: &str| ops.get(name).filter(|v| !v.is_null());
    let text = |name: &str| -> Result<Option<String>, String> {
        op(name)
            .map(|v| v.string().map(str::to_owned).map_err(|e| e.message))
            .transpose()
    };
    let key = key.to_owned();
    Ok(match kind {
        FilterKind::Select => text(EQ)?.map(|v| Condition::Equals {
            key,
            value: Param::Text(v),
        }),
        FilterKind::Boolean => op(EQ)
            .map(|v| v.boolean().map_err(|e| e.message))
            .transpose()?
            .map(|v| Condition::Equals {
                key,
                value: Param::Bool(v),
            }),
        FilterKind::Selects => text(CONTAINS)?.map(|value| Condition::Contains { key, value }),
        FilterKind::Reference | FilterKind::References => {
            let name = if kind == FilterKind::Reference {
                EQ
            } else {
                CONTAINS
            };
            match text(name)? {
                None => None,
                Some(v) => Some(Condition::References {
                    group: Uuid::parse_str(&v).map_err(|_| {
                        format!("`where.{key}.{name}` must be a translation group ID")
                    })?,
                    key,
                    multiple: kind == FilterKind::References,
                    content_type: target.unwrap_or_default().to_owned(),
                }),
            }
        }
        FilterKind::Int | FilterKind::Float | FilterKind::Date | FilterKind::DateTime => {
            let bound = |name: &str| -> Result<Option<Param>, String> {
                let Some(v) = op(name) else {
                    return Ok(None);
                };
                let invalid = |what: &str| format!("`where.{key}.{name}` must be {what}");
                Ok(Some(match kind {
                    FilterKind::Int => Param::Int(v.i64().map_err(|e| e.message)?),
                    FilterKind::Float => Param::Float(v.f64().map_err(|e| e.message)?),
                    FilterKind::Date => {
                        let s = v.string().map_err(|e| e.message)?;
                        if !datetime::valid_date(s) {
                            return Err(invalid("a date (YYYY-MM-DD)"));
                        }
                        Param::Text(s.to_owned())
                    }
                    _ => Param::Text(
                        datetime::normalize_datetime(v.string().map_err(|e| e.message)?)
                            .ok_or_else(|| invalid("a date and time with an offset (RFC 3339)"))?,
                    ),
                }))
            };
            let (gte, lte) = (bound(GTE)?, bound(LTE)?);
            let sort = if matches!(kind, FilterKind::Int | FilterKind::Float) {
                SortKind::Number
            } else {
                SortKind::Text
            };
            (gte.is_some() || lte.is_some()).then_some(Condition::Range {
                key,
                kind: sort,
                gte,
                lte,
            })
        }
    })
}

/// 組み立てた SQL の断片と、そのパラメーター。
#[derive(Debug, Default, PartialEq)]
pub struct Sql {
    /// `WHERE` に AND でつなぐ条件（先頭に ` AND ` を付けたもの。条件がなければ空）。
    pub filter: String,
    /// `ORDER BY` の先頭に置く並べ替え（末尾に `, ` を付けたもの。指定がなければ空）。
    pub order: String,
    pub params: Vec<Param>,
}

impl Sql {
    /// パラメーターを足し、そのプレースホルダー（`$n`）を返す。
    fn bind(&mut self, first: usize, p: Param) -> String {
        self.params.push(p);
        format!("${}", first + self.params.len() - 1)
    }
}

/// 公開中の版の値（`v.list_values`。`migrations/workspace/0030_list_values.sql`）の、数・文字列として読んだもの（型が合わなければ NULL）。
fn value_expr(key: &str, kind: SortKind) -> String {
    match kind {
        SortKind::Number => format!(
            "(CASE WHEN jsonb_typeof(v.list_values -> {key}) = 'number' THEN (v.list_values ->> {key})::numeric END)"
        ),
        // 書式をそろえた文字列なので、照合順序によらずバイト順で比べる
        SortKind::Text => format!(
            "(CASE WHEN jsonb_typeof(v.list_values -> {key}) = 'string' THEN v.list_values ->> {key} END) COLLATE \"C\""
        ),
    }
}

/// 範囲の端の値の型（列の読み方に合わせる）。
fn cast(kind: SortKind) -> &'static str {
    match kind {
        SortKind::Number => "::numeric",
        SortKind::Text => "::text COLLATE \"C\"",
    }
}

impl ListQuery {
    /// SQL の断片にする。プレースホルダーは `$first` から振る。
    /// 埋め込むのは定数とプレースホルダーだけで、フィールドの名前と値はすべてパラメーターにする。
    pub fn sql(&self, first: usize) -> Sql {
        let mut sql = Sql::default();
        let mut filter = String::new();
        for c in &self.conditions {
            let part = match c {
                Condition::Equals { key, value } => {
                    let k = sql.bind(first, Param::Text(key.clone()));
                    let ty = if matches!(value, Param::Bool(_)) {
                        "boolean"
                    } else {
                        "text"
                    };
                    let v = sql.bind(first, value.clone());
                    format!("v.list_values -> {k} = to_jsonb({v}::{ty})")
                }
                Condition::Contains { key, value } => {
                    let k = sql.bind(first, Param::Text(key.clone()));
                    let v = sql.bind(first, Param::Text(value.clone()));
                    format!("v.list_values -> {k} @> jsonb_build_array({v}::text)")
                }
                Condition::Range {
                    key,
                    kind,
                    gte,
                    lte,
                } => {
                    let k = sql.bind(first, Param::Text(key.clone()));
                    let expr = value_expr(&k, *kind);
                    let mut parts = Vec::new();
                    for (op, bound) in [(">=", gte), ("<=", lte)] {
                        if let Some(b) = bound {
                            let c = cast(*kind);
                            let p = sql.bind(first, b.clone());
                            parts.push(format!("{expr} {op} {p}{c}"));
                        }
                    }
                    parts.join(" AND ")
                }
                Condition::References {
                    key,
                    group,
                    multiple,
                    content_type,
                } => {
                    let k = sql.bind(first, Param::Text(key.clone()));
                    let g = sql.bind(first, Param::Text(group.hyphenated().to_string()));
                    let matches = if *multiple {
                        format!("v.list_values -> {k} @> jsonb_build_array({g}::text)")
                    } else {
                        format!("v.list_values -> {k} = to_jsonb({g}::text)")
                    };
                    let id = sql.bind(first, Param::Uuid(*group));
                    let ct = sql.bind(first, Param::Text(content_type.clone()));
                    // 配信で参照を解決するとき（data::resolve_groups）と同じ照合: 同じロケールの、削除されていない公開中の記事
                    format!(
                        "{matches} AND EXISTS (SELECT 1 FROM entries r JOIN versions rv ON rv.id = r.published_version_id
                           WHERE r.translation_group_id = {id}::uuid AND r.content_type = {ct}::text
                             AND r.locale = e.locale AND r.deleted_at IS NULL AND rv.fields IS NOT NULL)"
                    )
                }
            };
            let _ = write!(filter, " AND ({part})");
        }
        sql.filter = filter;
        if let Some(Sort { key, descending }) = &self.sort {
            let dir = if *descending { "DESC" } else { "ASC" };
            sql.order = match key {
                SortKey::PublishedAt => format!("v.published_at {dir}, "),
                SortKey::Field { key, kind } => {
                    let k = sql.bind(first, Param::Text(key.clone()));
                    format!("{} {dir} NULLS LAST, ", value_expr(&k, *kind))
                }
            };
        }
        sql
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_values_are_parameters() {
        let q = ListQuery {
            conditions: vec![
                Condition::Equals {
                    key: "category".into(),
                    value: Param::Text("x'); DROP TABLE entries; --".into()),
                },
                Condition::Range {
                    key: "rating".into(),
                    kind: SortKind::Number,
                    gte: Some(Param::Int(1)),
                    lte: None,
                },
            ],
            sort: Some(Sort {
                key: SortKey::Field {
                    key: "rating".into(),
                    kind: SortKind::Number,
                },
                descending: true,
            }),
        };
        let sql = q.sql(5);
        assert!(!sql.filter.contains("category") && !sql.filter.contains("DROP"));
        assert!(!sql.order.contains("rating"));
        assert!(sql.filter.contains("$5") && sql.filter.contains("$8"));
        assert!(
            sql.order
                .starts_with("(CASE WHEN jsonb_typeof(v.list_values -> $9)")
        );
        assert!(sql.order.ends_with("DESC NULLS LAST, "));
        assert_eq!(sql.params.len(), 5);
    }

    #[test]
    fn no_conditions_add_nothing() {
        assert_eq!(ListQuery::default().sql(5), Sql::default());
    }
}
