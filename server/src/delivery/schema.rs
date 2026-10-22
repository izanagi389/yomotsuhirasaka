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

use super::ai::{self, CatalogItem};
use super::data::{self, Entry};
use super::names::{
    self, AI_CATALOG_FIELD, AI_CATALOG_ITEM_TYPE, AI_MARKDOWN_FIELD, AI_TRAINING_OPT_OUTS_FIELD,
    ALTERNATE_TYPE, CORRECTION_FEED_ITEM_TYPE, CORRECTION_TYPE, CORRECTIONS_FIELD,
    DELETED_ANCHOR_TYPE, JSON_SCALAR, QUERY_TYPE, REDIRECT_TYPE, REDIRECTS_FIELD,
};
use crate::content::corrections::{self, Correction, DeletedAnchor, FeedItem};
use crate::content::freshness::{self, Freshness};
use crate::content::redirects::{self, Rule};
use crate::schema::manifest::{ContentType, FieldType, Manifest};

/// 一覧の `offset` の上限。
const MAX_OFFSET: i64 = 100_000;
/// 訂正フィードの件数の上限。
const FEED_LIMIT: i64 = 100;

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

/// 返す値の型 `T` の1つのフィールド（`get` が値を取り出す）。
fn value_field<T, F>(name: &str, ty: TypeRef, get: F) -> Field
where
    T: Send + Sync + 'static,
    F: Fn(&T) -> Option<GqlValue> + Send + Sync + 'static,
{
    Field::new(name, ty, move |ctx| {
        let v = ctx.parent_value.try_downcast_ref::<T>().map(&get);
        FieldFuture::new(async move { v })
    })
}

fn sections_value(sections: &[String]) -> Option<GqlValue> {
    Some(GqlValue::List(
        sections.iter().cloned().map(GqlValue::from).collect(),
    ))
}

/// 訂正履歴の1件の型（Q63）。説明の編集履歴と編集日時は出さない（INV-COR-04）。
fn correction_type() -> Object {
    Object::new(CORRECTION_TYPE)
        .description(
            "A version published as a correction. `sections` are the anchor IDs it affected.",
        )
        .field(value_field::<Correction, _>(
            "correctedAt",
            TypeRef::named_nn(TypeRef::STRING),
            |c| Some(GqlValue::from(c.corrected_at.clone())),
        ))
        .field(value_field::<Correction, _>(
            "description",
            TypeRef::named(TypeRef::STRING),
            |c| c.description.clone().map(GqlValue::from),
        ))
        .field(value_field::<Correction, _>(
            "sections",
            TypeRef::named_nn_list_nn(TypeRef::STRING),
            |c| sections_value(&c.sections),
        ))
}

/// 削除済みアンカーの1件の型。消えたセクションの本文は持たない（INV-COR-02）。
fn deleted_anchor_type() -> Object {
    Object::new(DELETED_ANCHOR_TYPE)
        .description("An anchor ID that no longer exists in the current version. `description` is set when it was removed by a correction.")
        .field(value_field::<DeletedAnchor, _>("anchor", TypeRef::named_nn(TypeRef::STRING), |d| {
            Some(GqlValue::from(d.anchor.clone()))
        }))
        .field(value_field::<DeletedAnchor, _>("deletedAt", TypeRef::named_nn(TypeRef::STRING), |d| {
            Some(GqlValue::from(d.deleted_at.clone()))
        }))
        .field(value_field::<DeletedAnchor, _>("description", TypeRef::named(TypeRef::STRING), |d| {
            d.description.clone().map(GqlValue::from)
        }))
}

/// 訂正フィードの1件の型（INV-COR-03）。
fn correction_feed_item_type() -> Object {
    Object::new(CORRECTION_FEED_ITEM_TYPE)
        .description("A correction of a published entry, newest first.")
        .field(value_field::<FeedItem, _>(
            "entryId",
            TypeRef::named_nn(TypeRef::ID),
            |i| Some(GqlValue::from(i.entry_id.to_string())),
        ))
        .field(value_field::<FeedItem, _>(
            "contentType",
            TypeRef::named_nn(TypeRef::STRING),
            |i| Some(GqlValue::from(i.content_type.clone())),
        ))
        .field(value_field::<FeedItem, _>(
            "url",
            TypeRef::named_nn(TypeRef::STRING),
            |i| Some(GqlValue::from(i.url.clone())),
        ))
        .field(value_field::<FeedItem, _>(
            "title",
            TypeRef::named(TypeRef::STRING),
            |i| i.title.clone().map(GqlValue::from),
        ))
        .field(value_field::<FeedItem, _>(
            "correctedAt",
            TypeRef::named_nn(TypeRef::STRING),
            |i| Some(GqlValue::from(i.corrected_at.clone())),
        ))
        .field(value_field::<FeedItem, _>(
            "description",
            TypeRef::named(TypeRef::STRING),
            |i| i.description.clone().map(GqlValue::from),
        ))
        .field(value_field::<FeedItem, _>(
            "sections",
            TypeRef::named_nn_list_nn(TypeRef::STRING),
            |i| sections_value(&i.sections),
        ))
}

/// 記事の訂正履歴・削除済みアンカー・鮮度（06-corrections-and-freshness.md）。返すたびに現在の状態から求める。
/// 鮮度は最終確認日と期限だけで、確認者とメモは出さない（INV-FRS-01）。
fn correction_fields(obj: Object, ct: &ContentType) -> Object {
    let corrections = Field::new(
        CORRECTIONS_FIELD,
        TypeRef::named_nn_list_nn(CORRECTION_TYPE),
        |ctx| {
            FieldFuture::new(async move {
                let e = parent(&ctx)?;
                let req = request(&ctx)?;
                let mut guard = req.tx.lock().await;
                let Some(tx) = guard.as_mut() else {
                    return Ok(Some(FieldValue::list(Vec::<FieldValue>::new())));
                };
                let out = corrections::entry_corrections(tx, e.id, &e.locale)
                    .await
                    .map_err(internal)?;
                Ok(Some(FieldValue::list(
                    out.into_iter().map(FieldValue::owned_any),
                )))
            })
        },
    )
    .description("Versions published as corrections, oldest first.");
    let deleted = Field::new(
        "deletedAnchors",
        TypeRef::named_nn_list_nn(DELETED_ANCHOR_TYPE),
        |ctx| {
            FieldFuture::new(async move {
                let e = parent(&ctx)?;
                let req = request(&ctx)?;
                let mut guard = req.tx.lock().await;
                let Some(tx) = guard.as_mut() else {
                    return Ok(Some(FieldValue::list(Vec::<FieldValue>::new())));
                };
                let out = corrections::deleted_anchors(tx, e.id, &e.locale)
                    .await
                    .map_err(internal)?;
                Ok(Some(FieldValue::list(
                    out.into_iter().map(FieldValue::owned_any),
                )))
            })
        },
    )
    .description("Anchor IDs removed from the entry, so that old links can show a notice.");
    let default_days = ct.review.as_ref().map(|r| r.interval_days);
    let fresh = |name: &str, ty: TypeRef, get: fn(Freshness) -> Option<GqlValue>| {
        Field::new(name, ty, move |ctx| {
            FieldFuture::new(async move {
                let e = parent(&ctx)?;
                let req = request(&ctx)?;
                let mut guard = req.tx.lock().await;
                let Some(tx) = guard.as_mut() else {
                    return Ok(None);
                };
                let f = freshness::get(tx, default_days, e.id)
                    .await
                    .map_err(internal)?;
                Ok(f.and_then(get))
            })
        })
    };
    obj.field(corrections)
        .field(deleted)
        .field(
            fresh("lastVerifiedAt", TypeRef::named(TypeRef::STRING), |f| {
                f.last_verified_at.map(GqlValue::from)
            })
            .description("When the content was last verified (or first published)."),
        )
        .field(
            fresh("reviewDueAt", TypeRef::named(TypeRef::STRING), |f| {
                f.review_due_at.map(GqlValue::from)
            })
            .description(
                "When the content should be verified again. Null when it has no review deadline.",
            ),
        )
        .field(fresh(
            "reviewExpired",
            TypeRef::named_nn(TypeRef::BOOLEAN),
            |f| Some(GqlValue::from(f.review_expired)),
        ))
}

/// 訂正フィード（INV-COR-03）。訂正として公開した版だけを、新しい順に返す。
fn corrections_feed_field(manifest: Arc<Manifest>) -> Field {
    Field::new(
        CORRECTIONS_FIELD,
        TypeRef::named_nn_list_nn(CORRECTION_FEED_ITEM_TYPE),
        move |ctx| {
            let manifest = manifest.clone();
            FieldFuture::new(async move {
                let locale = ctx.args.try_get("locale")?.string()?.to_owned();
                let limit = ctx
                    .args
                    .get("limit")
                    .and_then(|v| v.i64().ok())
                    .unwrap_or(FEED_LIMIT)
                    .clamp(0, FEED_LIMIT);
                let offset = ctx
                    .args
                    .get("offset")
                    .and_then(|v| v.i64().ok())
                    .unwrap_or(0)
                    .clamp(0, MAX_OFFSET);
                let req = request(&ctx)?;
                let mut guard = req.tx.lock().await;
                let Some(tx) = guard.as_mut() else {
                    return Ok(Some(FieldValue::list(Vec::<FieldValue>::new())));
                };
                let out = corrections::feed(tx, &manifest, &locale, limit, offset)
                    .await
                    .map_err(internal)?;
                Ok(Some(FieldValue::list(
                    out.into_iter().map(FieldValue::owned_any),
                )))
            })
        },
    )
    .argument(InputValue::new(
        "locale",
        TypeRef::named_nn(TypeRef::STRING),
    ))
    .argument(InputValue::new("limit", TypeRef::named(TypeRef::INT)))
    .argument(InputValue::new("offset", TypeRef::named(TypeRef::INT)))
    .description("Corrections of published entries in the locale, newest first.")
}

/// AI向けの素材（07-ai-delivery.md）: Markdown版、読了時間、学習への利用の拒否。スイッチは返すたびにいまの値を見る（INV-AIX-01）。
fn ai_fields(obj: Object, ct: &ContentType, manifest: Arc<Manifest>) -> Object {
    let markdown = Field::new(
        AI_MARKDOWN_FIELD,
        TypeRef::named(TypeRef::STRING),
        move |ctx| {
            let manifest = manifest.clone();
            FieldFuture::new(async move {
                let e = parent(&ctx)?;
                let req = request(&ctx)?;
                let mut guard = req.tx.lock().await;
                let Some(tx) = guard.as_mut() else {
                    return Ok(None);
                };
                let md = ai::markdown(tx, &manifest, &req.media_base, req.workspace_id, e)
                    .await
                    .map_err(internal)?;
                Ok(md.map(GqlValue::from))
            })
        },
    )
    .description(
        "The entry as Markdown for AI readers (null when the entry is not an AI material).",
    );
    let ct_for_time = ct.clone();
    obj.field(markdown)
        .field(
            Field::new("readingTime", TypeRef::named_nn(TypeRef::INT), move |ctx| {
                let v = parent(&ctx).map(|e| GqlValue::from(ai::reading_time(&ct_for_time, &e.rendered)));
                FieldFuture::new(async move { Ok(Some(v?)) })
            })
            .description("Estimated reading time in minutes (at least 1)."),
        )
        .field(
            value_field::<Entry, _>("aiTrainingOptOut", TypeRef::named_nn(TypeRef::BOOLEAN), |e| {
                Some(GqlValue::from(e.opt_out_of_ai_training))
            })
            .description("Whether the entry opts out of use for AI training (a signal to crawlers, not an enforcement)."),
        )
}

/// 目録の1件の型。
fn catalog_item_type() -> Object {
    Object::new(AI_CATALOG_ITEM_TYPE)
        .description("A published entry offered to AI readers.")
        .field(value_field::<CatalogItem, _>(
            "entryId",
            TypeRef::named_nn(TypeRef::ID),
            |i| Some(GqlValue::from(i.entry_id.to_string())),
        ))
        .field(value_field::<CatalogItem, _>(
            "contentType",
            TypeRef::named_nn(TypeRef::STRING),
            |i| Some(GqlValue::from(i.content_type.clone())),
        ))
        .field(value_field::<CatalogItem, _>(
            "url",
            TypeRef::named_nn(TypeRef::STRING),
            |i| Some(GqlValue::from(i.url.clone())),
        ))
        .field(value_field::<CatalogItem, _>(
            "title",
            TypeRef::named(TypeRef::STRING),
            |i| i.title.clone().map(GqlValue::from),
        ))
        .field(value_field::<CatalogItem, _>(
            "locale",
            TypeRef::named_nn(TypeRef::STRING),
            |i| Some(GqlValue::from(i.locale.clone())),
        ))
        .field(value_field::<CatalogItem, _>(
            "updatedAt",
            TypeRef::named_nn(TypeRef::STRING),
            |i| Some(GqlValue::from(i.updated_at.clone())),
        ))
}

/// 目録（INV-AIX-01）。
fn catalog_field(manifest: Arc<Manifest>) -> Field {
    Field::new(
        AI_CATALOG_FIELD,
        TypeRef::named_nn_list_nn(AI_CATALOG_ITEM_TYPE),
        move |ctx| {
            let manifest = manifest.clone();
            FieldFuture::new(async move {
                let locale = ctx.args.try_get("locale")?.string()?.to_owned();
                let limit = ctx
                    .args
                    .get("limit")
                    .and_then(|v| v.i64().ok())
                    .unwrap_or(ai::CATALOG_LIMIT)
                    .clamp(0, ai::CATALOG_LIMIT);
                let offset = ctx
                    .args
                    .get("offset")
                    .and_then(|v| v.i64().ok())
                    .unwrap_or(0)
                    .clamp(0, MAX_OFFSET);
                let req = request(&ctx)?;
                let mut guard = req.tx.lock().await;
                let Some(tx) = guard.as_mut() else {
                    return Ok(Some(FieldValue::list(Vec::<FieldValue>::new())));
                };
                let out = ai::catalog(tx, &manifest, &locale, limit, offset)
                    .await
                    .map_err(internal)?;
                Ok(Some(FieldValue::list(
                    out.into_iter().map(FieldValue::owned_any),
                )))
            })
        },
    )
    .argument(InputValue::new(
        "locale",
        TypeRef::named_nn(TypeRef::STRING),
    ))
    .argument(InputValue::new("limit", TypeRef::named(TypeRef::INT)))
    .argument(InputValue::new("offset", TypeRef::named(TypeRef::INT)))
    .description(
        "Published entries offered to AI readers in the locale, most recently updated first.",
    )
}

/// 学習への利用を拒否した記事のURL（すべてのロケール）。
fn training_opt_outs_field() -> Field {
    Field::new(
        AI_TRAINING_OPT_OUTS_FIELD,
        TypeRef::named_nn_list_nn(TypeRef::STRING),
        |ctx| {
            FieldFuture::new(async move {
                let req = request(&ctx)?;
                let mut guard = req.tx.lock().await;
                let Some(tx) = guard.as_mut() else {
                    return Ok(Some(GqlValue::List(Vec::new())));
                };
                let urls = ai::training_opt_outs(tx).await.map_err(internal)?;
                Ok(Some(GqlValue::List(
                    urls.into_iter().map(GqlValue::from).collect(),
                )))
            })
        },
    )
    .description("Public URLs of published entries that opt out of use for AI training.")
}

fn entry_type(ct: &ContentType, manifest: Arc<Manifest>) -> Object {
    let mut obj = system_fields(Object::new(names::type_name(&ct.name)));
    obj = correction_fields(obj, ct);
    obj = ai_fields(obj, ct, manifest);
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
        if f.kind == FieldType::Richtext {
            obj = obj
                .field(rendered_field(&f.name, Rendering::Html))
                .field(rendered_field(&f.name, Rendering::Markdown));
        }
    }
    obj
}

#[derive(Debug, Clone, Copy)]
enum Rendering {
    Html,
    Markdown,
}

/// 本文のフィールドを変換したもの（`<名前>Html`・`<名前>Markdown`）。公開のときに版ごとに作ったものを返す。
/// 画像の参照先はその時点のメディアのURLに置き換え、HTMLは返すたびに無害化する（INV-CNV-01）。
/// フィールドの名前は英小文字・数字・`_` だけなので、大文字を含むこの名前が他のフィールドとぶつかることはない。
fn rendered_field(field: &str, kind: Rendering) -> Field {
    let (suffix, key) = match kind {
        Rendering::Html => ("Html", "html"),
        Rendering::Markdown => ("Markdown", "markdown"),
    };
    let name = field.to_owned();
    Field::new(
        format!("{field}{suffix}"),
        TypeRef::named(TypeRef::STRING),
        move |ctx| {
            let name = name.clone();
            FieldFuture::new(async move {
                let e = parent(&ctx)?;
                let Some(text) = e
                    .rendered
                    .get(&name)
                    .and_then(|r| r.get(key))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                else {
                    return Ok(None);
                };
                let req = request(&ctx)?;
                let mut guard = req.tx.lock().await;
                let Some(tx) = guard.as_mut() else {
                    return Ok(None);
                };
                let text = data::resolve_media_refs(tx, &req.media_base, req.workspace_id, &text)
                    .await
                    .map_err(internal)?;
                let text = match kind {
                    Rendering::Html => crate::content::html::sanitize(&text),
                    Rendering::Markdown => text,
                };
                Ok(Some(GqlValue::from(text)))
            })
        },
    )
    .description(match kind {
        Rendering::Html => "The rich text field converted to HTML (sanitized).",
        Rendering::Markdown => "The rich text field converted to Markdown.",
    })
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

/// リダイレクト一覧の1件の型。
fn redirect_type() -> Object {
    Object::new(REDIRECT_TYPE)
        .description("A redirect from an old public URL. `to` is null and `status` is 410 when the entry was deleted.")
        .field(Field::new("from", TypeRef::named_nn(TypeRef::STRING), |ctx| {
            let v = ctx.parent_value.try_downcast_ref::<Rule>().map(|r| r.from.clone());
            FieldFuture::new(async move { Ok(Some(GqlValue::from(v?))) })
        }))
        .field(Field::new("to", TypeRef::named(TypeRef::STRING), |ctx| {
            let v = ctx.parent_value.try_downcast_ref::<Rule>().map(|r| r.to.clone());
            FieldFuture::new(async move { Ok(v?.map(GqlValue::from)) })
        }))
        .field(Field::new("status", TypeRef::named_nn(TypeRef::INT), |ctx| {
            let v = ctx.parent_value.try_downcast_ref::<Rule>().map(|r| i32::from(r.status));
            FieldFuture::new(async move { Ok(Some(GqlValue::from(v?))) })
        }))
}

/// リダイレクト一覧（Q40、INV-RED-01, 02）。返すたびに、その時点の記事の状態から作り直す。
fn redirects_field() -> Field {
    Field::new(
        REDIRECTS_FIELD,
        TypeRef::named_nn_list_nn(REDIRECT_TYPE),
        |ctx| {
            FieldFuture::new(async move {
                let req = request(&ctx)?;
                let mut guard = req.tx.lock().await;
                let Some(tx) = guard.as_mut() else {
                    return Ok(Some(FieldValue::list(Vec::<FieldValue>::new())));
                };
                let rules = redirects::list(tx).await.map_err(internal)?;
                Ok(Some(FieldValue::list(
                    rules.into_iter().map(FieldValue::owned_any),
                )))
            })
        },
    )
    .description("Redirects from old public URLs, resolved to the current URL (no chains).")
}

/// マニフェストからスキーマを組み立てる。
pub fn build(
    manifest: &Manifest,
    introspection: bool,
    limits: QueryLimits,
) -> anyhow::Result<Schema> {
    let shared = Arc::new(manifest.clone());
    let mut query = Object::new(QUERY_TYPE)
        .field(redirects_field())
        .field(corrections_feed_field(shared.clone()))
        .field(catalog_field(shared.clone()))
        .field(training_opt_outs_field());
    for ct in &manifest.content_types {
        query = root_fields(query, ct);
    }
    let mut builder = Schema::build(QUERY_TYPE, None, None)
        .register(
            Scalar::new(JSON_SCALAR).description("A JSON value (rich text bodies as Tiptap JSON)."),
        )
        .register(alternate_type())
        .register(redirect_type())
        .register(correction_type())
        .register(deleted_anchor_type())
        .register(correction_feed_item_type())
        .register(catalog_item_type())
        .limit_depth(limits.depth)
        .limit_complexity(limits.complexity)
        .disable_suggestions();
    for ct in &manifest.content_types {
        builder = builder.register(entry_type(ct, shared.clone()));
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
