//! 配信APIの、テンプレートのフィールドの GraphQL のフィールド（02-schema-and-delivery-api.md「フィールドの型」）。
//!
//! 記事の型と、グループの各件の型の両方で使う（どちらも「フィールド名 → 値」の JSON を持つ）。
//! - 選択肢・日付・日時は String（複数を選べる選択肢は [String!]!）で返す（2026-11-25、Q14）。日時は UTC。
//! - 公開中の版の値は、公開したときの定義に従う。いまの定義の型に合わない値（グループの中のフィールドの型を変えた後の
//!   古い版など）は `null` として返し、配信を止めない。

use async_graphql::Value as GqlValue;
use async_graphql::dynamic::{Field, FieldFuture, FieldValue, Object, TypeRef};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::data::{self, Entry};
use super::names::{self, IMAGE_TYPE, JSON_SCALAR};
use super::schema::{internal, request};
use crate::schema::manifest::{Field as TemplateField, FieldType, GROUP_ITEM_ID_KEY};

/// テンプレートのフィールドの値を持つもの（記事と、グループの各件）。
pub(super) trait Values: Send + Sync + 'static {
    fn values(&self) -> &Map<String, Value>;
    /// 参照を解決するロケール（グループの中の参照は、その記事のロケールで解決する）。
    fn locale(&self) -> &str;
}

impl Values for Entry {
    fn values(&self) -> &Map<String, Value> {
        &self.fields
    }
    fn locale(&self) -> &str {
        &self.locale
    }
}

/// グループの1件。
struct GroupItem {
    values: Map<String, Value>,
    locale: String,
}

impl Values for GroupItem {
    fn values(&self) -> &Map<String, Value> {
        &self.values
    }
    fn locale(&self) -> &str {
        &self.locale
    }
}

/// 親の値から、そのフィールドの値を読んで GraphQL の値にする（同期で済むもの）。
fn plain<T, F>(name: &str, ty: TypeRef, convert: F) -> Field
where
    T: Values,
    F: Fn(&Value) -> Option<GqlValue> + Send + Sync + 'static,
{
    let key = name.to_owned();
    Field::new(name, ty, move |ctx| {
        let v = ctx
            .parent_value
            .try_downcast_ref::<T>()
            .map(|p| convert(p.values().get(&key).unwrap_or(&Value::Null)));
        FieldFuture::new(async move { v })
    })
}

fn string(v: &Value) -> Option<GqlValue> {
    v.as_str().map(GqlValue::from)
}

/// 文字列の配列（値がなければ空の配列）。
fn strings(v: &Value) -> GqlValue {
    GqlValue::List(v.as_array().map_or_else(Vec::new, |a| {
        a.iter()
            .filter_map(Value::as_str)
            .map(GqlValue::from)
            .collect()
    }))
}

/// テンプレートのフィールドの GraphQL のフィールド。
/// グループなら、その各件の型も `extra` に足す（型の名前は `<コンテンツタイプの型><フィールド>Item`）。
pub(super) fn field<T: Values>(
    content_type: &str,
    f: &TemplateField,
    extra: &mut Vec<Object>,
) -> Field {
    let field = match f.kind {
        FieldType::Text | FieldType::Slug | FieldType::Date | FieldType::Datetime => {
            plain::<T, _>(&f.name, TypeRef::named(TypeRef::STRING), string)
        }
        FieldType::Select if f.multiple => {
            plain::<T, _>(&f.name, TypeRef::named_nn_list_nn(TypeRef::STRING), |v| {
                Some(strings(v))
            })
        }
        FieldType::Select => plain::<T, _>(&f.name, TypeRef::named(TypeRef::STRING), string),
        FieldType::Number if f.integer => {
            plain::<T, _>(&f.name, TypeRef::named(TypeRef::INT), |v| {
                v.as_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .map(GqlValue::from)
            })
        }
        FieldType::Number => plain::<T, _>(&f.name, TypeRef::named(TypeRef::FLOAT), |v| {
            v.as_f64().map(GqlValue::from)
        }),
        FieldType::Boolean => plain::<T, _>(&f.name, TypeRef::named(TypeRef::BOOLEAN), |v| {
            v.as_bool().map(GqlValue::from)
        }),
        FieldType::Image => image_field::<T>(&f.name),
        FieldType::Reference => reference_field::<T>(f),
        FieldType::Group => group_field::<T>(content_type, f, extra),
        FieldType::Richtext => richtext_field::<T>(&f.name),
    };
    match &f.description {
        Some(d) => field.description(d.clone()),
        None => field,
    }
}

/// 本文のフィールド（Tiptap の JSON。画像の参照先は、その時点のメディアのURLにする）。
fn richtext_field<T: Values>(name: &str) -> Field {
    let key = name.to_owned();
    Field::new(name, TypeRef::named(JSON_SCALAR), move |ctx| {
        let key = key.clone();
        FieldFuture::new(async move {
            let p = ctx.parent_value.try_downcast_ref::<T>()?;
            let Some(doc) = p.values().get(&key).filter(|v| v.is_object()).cloned() else {
                return Ok(None);
            };
            let req = request(&ctx)?;
            let mut guard = req.tx.lock().await;
            let Some(tx) = guard.as_mut() else {
                return Ok(None);
            };
            let doc = data::with_media(tx, &req.media_base, req.workspace_id, doc)
                .await
                .map_err(internal)?;
            Ok(Some(FieldValue::value(
                GqlValue::from_json(doc).map_err(|e| internal(e.into()))?,
            )))
        })
    })
}

/// 画像のフィールド: メディアが削除されていれば null（INV-IMG-02）。
fn image_field<T: Values>(name: &str) -> Field {
    let key = name.to_owned();
    Field::new(name, TypeRef::named(IMAGE_TYPE), move |ctx| {
        let key = key.clone();
        FieldFuture::new(async move {
            let p = ctx.parent_value.try_downcast_ref::<T>()?;
            let Some(value) = p.values().get(&key).filter(|v| v.is_object()) else {
                return Ok(None);
            };
            let req = request(&ctx)?;
            let mut guard = req.tx.lock().await;
            let Some(tx) = guard.as_mut() else {
                return Ok(None);
            };
            let image = data::image(tx, &req.media_base, req.workspace_id, value)
                .await
                .map_err(internal)?;
            Ok(image.map(FieldValue::owned_any))
        })
    })
}

/// 参照のフィールド: 参照先の翻訳グループの、同じロケールの公開中の記事。
fn reference_field<T: Values>(f: &TemplateField) -> Field {
    let key = f.name.clone();
    let target_ct = f.reference_to.clone().unwrap_or_default();
    let target = names::type_name(&target_ct);
    let multiple = f.multiple;
    let ty = if multiple {
        TypeRef::named_nn_list_nn(target)
    } else {
        TypeRef::named(target)
    };
    Field::new(&f.name, ty, move |ctx| {
        let (key, target_ct) = (key.clone(), target_ct.clone());
        FieldFuture::new(async move {
            let p = ctx.parent_value.try_downcast_ref::<T>()?;
            let groups: Vec<Uuid> = match p.values().get(&key) {
                Some(Value::Array(a)) => a
                    .iter()
                    .filter_map(Value::as_str)
                    .filter_map(|s| Uuid::parse_str(s).ok())
                    .collect(),
                Some(Value::String(s)) => Uuid::parse_str(s).ok().into_iter().collect(),
                _ => Vec::new(),
            };
            let req = request(&ctx)?;
            let mut guard = req.tx.lock().await;
            let resolved = match guard.as_mut() {
                Some(tx) => data::resolve_groups(tx, &target_ct, p.locale(), &groups)
                    .await
                    .map_err(internal)?,
                None => Vec::new(),
            };
            drop(guard);
            req.charge(resolved.len())?;
            if multiple {
                Ok(Some(FieldValue::list(
                    resolved.into_iter().map(FieldValue::owned_any),
                )))
            } else {
                Ok(resolved.into_iter().next().map(FieldValue::owned_any))
            }
        })
    })
}

/// グループのフィールド: 各件を、固定のID（`id`）と中のフィールドを持つ型で返す（Q10、Q16）。
fn group_field<T: Values>(content_type: &str, f: &TemplateField, extra: &mut Vec<Object>) -> Field {
    let item_type = names::group_item_type(content_type, &f.name);
    let mut obj = Object::new(&item_type)
        .description(format!(
            "An item of the group field `{}`. `id` stays the same when items are edited or reordered.",
            f.name
        ))
        .field(plain::<GroupItem, _>(
            GROUP_ITEM_ID_KEY,
            TypeRef::named_nn(TypeRef::ID),
            string,
        ));
    for inner in &f.fields {
        obj = obj.field(field::<GroupItem>(content_type, inner, extra));
    }
    extra.push(obj);

    let key = f.name.clone();
    Field::new(&f.name, TypeRef::named_nn_list_nn(item_type), move |ctx| {
        let items = ctx.parent_value.try_downcast_ref::<T>().map(|p| {
            crate::content::groups::items(p.values().get(&key))
                // ID のない件は返さない（保存のときに必ず振るので、ふつうは起きない）
                .filter(|item| item.get(GROUP_ITEM_ID_KEY).is_some_and(Value::is_string))
                .map(|item| GroupItem {
                    values: item.clone(),
                    locale: p.locale().to_owned(),
                })
                .collect::<Vec<_>>()
        });
        FieldFuture::new(async move {
            Ok(Some(FieldValue::list(
                items?.into_iter().map(FieldValue::owned_any),
            )))
        })
    })
}
