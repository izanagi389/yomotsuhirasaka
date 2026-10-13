//! テンプレートの版ごとのGraphQLのスキーマ（動的スキーマ。02-schema-and-delivery-api.md、Q55, Q130）。
//!
//! - スキーマはマニフェストから組み立て、（テンプレート, 版）ごとにメモリに持つ。ワークスペースには、適用済みの版のスキーマを使う
//!   （移行待ちの間も配信を続ける）。
//! - 本番では、イントロスペクションを無効にする（INV-GQL-04）。深さと複雑さに上限を設ける（INV-GQL-06）。
//! - 一覧の件数は、コンテンツタイプの `listLimit` を超えない（INV-GQL-05）。
//! - リゾルバーのエラーには内部の詳細を書かない。
//! - `devNote` はどこにも出さない（Q51）。

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_graphql::Value as GqlValue;
use async_graphql::dynamic::{
    Field, FieldFuture, FieldValue, InputValue, Object, ResolverContext, Scalar, Schema, TypeRef,
};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use tokio::sync::Mutex;
use uuid::Uuid;

use super::data::{self, Entry};
use super::names::{self, ALTERNATE_TYPE, JSON_SCALAR, QUERY_TYPE};
use crate::schema::manifest::{ContentType, FieldType, Manifest};

/// 一覧の `offset` の上限。
const MAX_OFFSET: i64 = 100_000;

/// 1回のリクエストで使うもの（リクエストのデータとして渡す）。
pub struct RequestData {
    /// ワークスペースのロールに切り替えた読み取りのトランザクション。None なら、データを読まずに検証だけを行う（登録のとき）。
    pub tx: Mutex<Option<Transaction<'static, Postgres>>>,
    pub workspace_id: Uuid,
    pub media_base: String,
}

/// 深さと複雑さの上限。
#[derive(Debug, Clone, Copy)]
pub struct QueryLimits {
    pub depth: usize,
    pub complexity: usize,
}

fn internal(e: anyhow::Error) -> async_graphql::Error {
    tracing::error!("delivery resolver failed: {e:#}");
    async_graphql::Error::new("internal error")
}

fn parent<'a>(ctx: &'a ResolverContext<'a>) -> async_graphql::Result<&'a Entry> {
    ctx.parent_value.try_downcast_ref::<Entry>()
}

fn request<'a>(ctx: &'a ResolverContext<'a>) -> async_graphql::Result<&'a RequestData> {
    Ok(ctx.data::<Arc<RequestData>>()?.as_ref())
}

fn list_of(entries: Vec<Entry>) -> FieldValue<'static> {
    FieldValue::list(entries.into_iter().map(FieldValue::owned_any))
}

/// 記事の型に共通の、CMSが決めるフィールド。
fn system_fields(obj: Object) -> Object {
    fn text<F>(name: &str, ty: TypeRef, get: F) -> Field
    where
        F: Fn(&Entry) -> String + Send + Sync + 'static,
    {
        Field::new(name, ty, move |ctx| {
            let v = parent(&ctx).map(&get);
            FieldFuture::new(async move { Ok(Some(GqlValue::from(v?))) })
        })
    }
    obj.field(text("id", TypeRef::named_nn(TypeRef::ID), |e| {
        e.id.to_string()
    }))
    .field(text("locale", TypeRef::named_nn(TypeRef::STRING), |e| {
        e.locale.clone()
    }))
    .field(text("url", TypeRef::named_nn(TypeRef::STRING), |e| {
        e.url.clone()
    }))
    .field(text(
        "publishedAt",
        TypeRef::named_nn(TypeRef::STRING),
        |e| e.published_at.clone(),
    ))
    .field(text(
        "firstPublishedAt",
        TypeRef::named_nn(TypeRef::STRING),
        |e| e.first_published_at.clone(),
    ))
    .field(text(
        "translationGroupId",
        TypeRef::named_nn(TypeRef::ID),
        |e| e.group_id.to_string(),
    ))
}

fn alternate_type() -> Object {
    let t = Object::new(ALTERNATE_TYPE)
        .description("A published entry of the same translation group in another locale.");
    fn text<F>(name: &str, ty: TypeRef, get: F) -> Field
    where
        F: Fn(&Entry) -> String + Send + Sync + 'static,
    {
        Field::new(name, ty, move |ctx| {
            let v = parent(&ctx).map(&get);
            FieldFuture::new(async move { Ok(Some(GqlValue::from(v?))) })
        })
    }
    t.field(text("id", TypeRef::named_nn(TypeRef::ID), |e| {
        e.id.to_string()
    }))
    .field(text("locale", TypeRef::named_nn(TypeRef::STRING), |e| {
        e.locale.clone()
    }))
    .field(text("url", TypeRef::named_nn(TypeRef::STRING), |e| {
        e.url.clone()
    }))
}

fn entry_type(ct: &ContentType) -> Object {
    let mut obj = system_fields(Object::new(names::type_name(&ct.name)));
    if let Some(d) = &ct.description {
        obj = obj.description(d.clone());
    }
    obj = obj.field(Field::new(
        "alternates",
        TypeRef::named_nn_list_nn(ALTERNATE_TYPE),
        |ctx| {
            FieldFuture::new(async move {
                let e = parent(&ctx)?;
                let req = request(&ctx)?;
                let mut guard = req.tx.lock().await;
                let Some(tx) = guard.as_mut() else {
                    return Ok(Some(list_of(Vec::new())));
                };
                let out = data::alternates(tx, e.group_id, e.id)
                    .await
                    .map_err(internal)?;
                Ok(Some(list_of(out)))
            })
        },
    ));
    for f in &ct.fields {
        let name = f.name.clone();
        let mut field = match f.kind {
            FieldType::Text | FieldType::Slug => {
                Field::new(&f.name, TypeRef::named(TypeRef::STRING), move |ctx| {
                    let v = parent(&ctx).map(|e| {
                        e.fields
                            .get(&name)
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    });
                    FieldFuture::new(async move { Ok(v?.map(GqlValue::from)) })
                })
            }
            FieldType::Richtext => Field::new(&f.name, TypeRef::named(JSON_SCALAR), move |ctx| {
                let name = name.clone();
                FieldFuture::new(async move {
                    let e = parent(&ctx)?;
                    let Some(doc) = e.fields.get(&name).filter(|v| v.is_object()).cloned() else {
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
            }),
            FieldType::Reference => {
                let target_ct = f.reference_to.clone().unwrap_or_default();
                let target = names::type_name(&target_ct);
                let multiple = f.multiple;
                let ty = if multiple {
                    TypeRef::named_nn_list_nn(target)
                } else {
                    TypeRef::named(target)
                };
                Field::new(&f.name, ty, move |ctx| {
                    let (name, target_ct) = (name.clone(), target_ct.clone());
                    FieldFuture::new(async move {
                        let e = parent(&ctx)?;
                        let groups: Vec<Uuid> = match e.fields.get(&name) {
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
                            Some(tx) => data::resolve_groups(tx, &target_ct, &e.locale, &groups)
                                .await
                                .map_err(internal)?,
                            None => Vec::new(),
                        };
                        if multiple {
                            Ok(Some(list_of(resolved)))
                        } else {
                            Ok(resolved.into_iter().next().map(FieldValue::owned_any))
                        }
                    })
                })
            }
        };
        if let Some(d) = &f.description {
            field = field.description(d.clone());
        }
        obj = obj.field(field);
    }
    obj
}

fn root_fields(query: Object, ct: &ContentType) -> Object {
    let ty = names::type_name(&ct.name);
    let name = ct.name.clone();
    let single = Field::new(names::single(&ct.name), TypeRef::named(&ty), move |ctx| {
        let name = name.clone();
        FieldFuture::new(async move {
            let id = ctx.args.try_get("id")?.string()?.to_owned();
            let locale = ctx
                .args
                .get("locale")
                .and_then(|v| v.string().ok().map(str::to_owned));
            let Ok(id) = Uuid::parse_str(&id) else {
                return Ok(None);
            };
            let req = request(&ctx)?;
            let mut guard = req.tx.lock().await;
            let Some(tx) = guard.as_mut() else {
                return Ok(None);
            };
            let found = data::by_id(tx, &name, id).await.map_err(internal)?;
            Ok(found
                .filter(|e| locale.as_deref().is_none_or(|l| l == e.locale))
                .map(FieldValue::owned_any))
        })
    })
    .argument(InputValue::new("id", TypeRef::named_nn(TypeRef::ID)))
    .argument(InputValue::new("locale", TypeRef::named(TypeRef::STRING)));

    let name = ct.name.clone();
    let slug_field = ct
        .fields
        .iter()
        .find(|f| f.kind == FieldType::Slug)
        .map(|f| f.name.clone());
    let by_slug = Field::new(names::by_slug(&ct.name), TypeRef::named(&ty), move |ctx| {
        let (name, slug_field) = (name.clone(), slug_field.clone());
        FieldFuture::new(async move {
            let slug = ctx.args.try_get("slug")?.string()?.to_owned();
            let locale = ctx.args.try_get("locale")?.string()?.to_owned();
            let req = request(&ctx)?;
            let mut guard = req.tx.lock().await;
            let (Some(tx), Some(slug_field)) = (guard.as_mut(), slug_field) else {
                return Ok(None);
            };
            let found = data::by_slug(tx, &name, &slug_field, &slug, &locale)
                .await
                .map_err(internal)?;
            Ok(found.map(FieldValue::owned_any))
        })
    })
    .argument(InputValue::new("slug", TypeRef::named_nn(TypeRef::STRING)))
    .argument(InputValue::new(
        "locale",
        TypeRef::named_nn(TypeRef::STRING),
    ));

    let name = ct.name.clone();
    let max = i64::from(ct.list_limit);
    let list = Field::new(
        names::list(&ct.name),
        TypeRef::named_nn_list_nn(&ty),
        move |ctx| {
            let name = name.clone();
            FieldFuture::new(async move {
                let locale = ctx.args.try_get("locale")?.string()?.to_owned();
                // 件数は、指定にかかわらずコンテンツタイプの上限まで（INV-GQL-05）
                let limit = ctx
                    .args
                    .get("limit")
                    .and_then(|v| v.i64().ok())
                    .unwrap_or(max)
                    .clamp(0, max);
                let offset = ctx
                    .args
                    .get("offset")
                    .and_then(|v| v.i64().ok())
                    .unwrap_or(0)
                    .clamp(0, MAX_OFFSET);
                let req = request(&ctx)?;
                let mut guard = req.tx.lock().await;
                let Some(tx) = guard.as_mut() else {
                    return Ok(Some(list_of(Vec::new())));
                };
                let out = data::list(tx, &name, &locale, limit, offset)
                    .await
                    .map_err(internal)?;
                Ok(Some(list_of(out)))
            })
        },
    )
    .argument(InputValue::new(
        "locale",
        TypeRef::named_nn(TypeRef::STRING),
    ))
    .argument(InputValue::new("limit", TypeRef::named(TypeRef::INT)))
    .argument(InputValue::new("offset", TypeRef::named(TypeRef::INT)));

    query.field(single).field(by_slug).field(list)
}

/// マニフェストからスキーマを組み立てる。
pub fn build(
    manifest: &Manifest,
    introspection: bool,
    limits: QueryLimits,
) -> anyhow::Result<Schema> {
    let mut query = Object::new(QUERY_TYPE);
    for ct in &manifest.content_types {
        query = root_fields(query, ct);
    }
    let mut builder = Schema::build(QUERY_TYPE, None, None)
        .register(
            Scalar::new(JSON_SCALAR).description("A JSON value (rich text bodies as Tiptap JSON)."),
        )
        .register(alternate_type())
        .limit_depth(limits.depth)
        .limit_complexity(limits.complexity)
        .disable_suggestions();
    for ct in &manifest.content_types {
        builder = builder.register(entry_type(ct));
    }
    if !introspection {
        builder = builder.disable_introspection();
    }
    builder = builder.register(query);
    Ok(builder.finish()?)
}

/// （テンプレート, 版）ごとのスキーマ。
#[derive(Default)]
pub struct Cache {
    schemas: RwLock<HashMap<(String, i32), Arc<Schema>>>,
}

impl Cache {
    /// 持っていればそれを、なければ組み立てて返す。
    pub fn get_or_build(
        &self,
        template: &str,
        version: i32,
        manifest: &Manifest,
        introspection: bool,
        limits: QueryLimits,
    ) -> anyhow::Result<Arc<Schema>> {
        let key = (template.to_owned(), version);
        if let Some(s) = self.schemas.read().expect("schema cache lock").get(&key) {
            return Ok(s.clone());
        }
        let schema = Arc::new(build(manifest, introspection, limits)?);
        self.schemas
            .write()
            .expect("schema cache lock")
            .insert(key, schema.clone());
        Ok(schema)
    }
}
