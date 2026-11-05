//! 配信APIの回答API（M4、09-qa-chatbot.md）。GraphQL の型と、`answer`（まとめて返す Query）・
//! `answerStream`（判定先行のストリーミングの Subscription。SSE で配信する。`http::delivery`）のリゾルバー。
//!
//! - 回答の中身（質問の検査・根拠・予算・LLMの回答の検査）は `crate::answer`。ここは型に詰めるだけ。
//! - 回答はお金がかかって共有できないので、POST だけで受け付け、`answer` のスコープのキーを求める。どちらも
//!   `http::delivery` が実行の前に確かめる。
//! - 回答できなかった理由は、エラーの `extensions.code`（`crate::answer::Refusal::code`）で返す。
//! - 回答の読み取りは、配信APIのリクエストのトランザクションを使わず、自分の短いトランザクションで行う
//!   （LLMの応答を待つ間、DBの接続を持ち続けないため）。

use std::sync::Arc;

use async_graphql::dynamic::{
    Enum, EnumItem, Field, FieldFuture, FieldValue, InputValue, Object, ResolverContext,
    SubscriptionField, SubscriptionFieldFuture, TypeRef, Union,
};
use async_graphql::futures_util::{StreamExt as _, stream};
use async_graphql::{ErrorExtensions as _, Value as GqlValue};

use super::names::{
    ANSWER_DELTA_TYPE, ANSWER_EVENT_TYPE, ANSWER_FIELD, ANSWER_FINAL_TYPE, ANSWER_SOURCE_TYPE,
    ANSWER_STATUS_TYPE, ANSWER_STREAM_FIELD, ANSWER_TYPE, ANSWER_VERDICT_TYPE, VISITOR_ID_ARG,
};
use super::schema::{internal, request, value_field};
use crate::answer::{self, Answer, Event, Refusal, Source, Status};
use crate::schema::manifest::Manifest;
use crate::workspace_db;

/// 判定のイベントの値。
struct VerdictValue {
    answerable: bool,
    sources: Vec<Source>,
}

/// 本文の断片のイベントの値。
struct DeltaValue(String);

fn refusal_error(r: Refusal) -> async_graphql::Error {
    async_graphql::Error::new(r.message()).extend_with(|_, e| e.set("code", r.code()))
}

/// 回答の依頼を作る（質問・ロケールの引数と、ワークスペースのDB）。検証だけのとき（登録）は None。
async fn answer_request(
    ctx: &ResolverContext<'_>,
    manifest: Arc<Manifest>,
    streaming: bool,
) -> async_graphql::Result<Option<(crate::http::AppState, answer::Request)>> {
    let question = ctx.args.try_get("question")?.string()?.to_owned();
    let locale = ctx.args.try_get("locale")?.string()?.to_owned();
    let req = request(ctx)?;
    let Some(app) = req.app.clone() else {
        return Ok(None);
    };
    let db = workspace_db::lookup(&app.pool, req.workspace_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| internal(anyhow::anyhow!("workspace not found")))?;
    Ok(Some((
        app,
        answer::Request {
            workspace_id: req.workspace_id,
            db,
            manifest,
            question,
            locale,
            streaming,
        },
    )))
}

fn arguments(field: Field) -> Field {
    field
        .argument(InputValue::new(
            "question",
            TypeRef::named_nn(TypeRef::STRING),
        ))
        .argument(InputValue::new(
            "locale",
            TypeRef::named_nn(TypeRef::STRING),
        ))
        .argument(visitor_arg())
}

/// 訪問者の識別子の引数。回数制限は、実行の前に配信APIの入口（http::delivery）で数える。ここでは値を使わない。
fn visitor_arg() -> InputValue {
    InputValue::new(VISITOR_ID_ARG, TypeRef::named(TypeRef::STRING)).description(
        "An opaque identifier of the visitor (hash it on your relay server), up to 128 characters. \
         Used only for per-visitor rate limits; never stored.",
    )
}

/// `answer(question, locale)`: 回答をまとめて返す。
pub fn answer_field(manifest: Arc<Manifest>) -> Field {
    let field = Field::new(ANSWER_FIELD, TypeRef::named_nn(ANSWER_TYPE), move |ctx| {
        let manifest = manifest.clone();
        FieldFuture::new(async move {
            let Some((app, req)) = answer_request(&ctx, manifest, false).await? else {
                // 検証だけのとき（登録）は、LLMを呼ばない
                return Ok(Some(FieldValue::owned_any(Answer {
                    status: Status::NoInformation,
                    text: String::new(),
                    truncated: false,
                    sources: Vec::new(),
                    cached: false,
                })));
            };
            let mut rx = answer::start(app, req);
            while let Some(item) = rx.recv().await {
                match item {
                    Ok(Event::Final(a)) => return Ok(Some(FieldValue::owned_any(a))),
                    Ok(_) => {}
                    Err(r) => return Err(refusal_error(r)),
                }
            }
            Err(internal(anyhow::anyhow!("answer ended without a result")))
        })
    })
    .description(
        "Answer a visitor's question from published entries, with sources. POST only; the API key needs the answer scope. \
         The text is Markdown: escape it before showing it.",
    );
    arguments(field)
}

/// `answerStream(question, locale)`: 判定 → 本文の断片 → 最終判定の順に返す。
pub fn answer_stream_field(manifest: Arc<Manifest>) -> SubscriptionField {
    SubscriptionField::new(
        ANSWER_STREAM_FIELD,
        TypeRef::named_nn(ANSWER_EVENT_TYPE),
        move |ctx| {
            let manifest = manifest.clone();
            SubscriptionFieldFuture::new(async move {
                let Some((app, req)) = answer_request(&ctx, manifest, true).await? else {
                    // 検証だけのとき（登録）は、何も流さない
                    return Ok(stream::empty().boxed());
                };
                let rx = answer::start(app, req);
                let events = stream::unfold(rx, |mut rx| async move {
                    rx.recv().await.map(|item| (item, rx))
                })
                .map(|item| match item {
                    Ok(Event::Verdict {
                        answerable,
                        sources,
                    }) => Ok(FieldValue::owned_any(VerdictValue {
                        answerable,
                        sources,
                    })
                    .with_type(ANSWER_VERDICT_TYPE)),
                    Ok(Event::Delta(text)) => {
                        Ok(FieldValue::owned_any(DeltaValue(text)).with_type(ANSWER_DELTA_TYPE))
                    }
                    Ok(Event::Final(a)) => Ok(FieldValue::owned_any(a).with_type(ANSWER_FINAL_TYPE)),
                    Err(r) => Err(refusal_error(r)),
                });
                Ok(events.boxed())
            })
        },
    )
    .description(
        "Answer a visitor's question as a stream: a verdict first, then text deltas (only when answerable), then a final verdict. \
         Show the final text and status; NO_INFORMATION replaces anything streamed before. POST over SSE only.",
    )
    .argument(InputValue::new(
        "question",
        TypeRef::named_nn(TypeRef::STRING),
    ))
    .argument(InputValue::new("locale", TypeRef::named_nn(TypeRef::STRING)))
    .argument(visitor_arg())
}

/// 出典の一覧のフィールド（`sources`）。
fn sources_field<T: Send + Sync + 'static>(get: fn(&T) -> &Vec<Source>) -> Field {
    Field::new(
        "sources",
        TypeRef::named_nn_list_nn(ANSWER_SOURCE_TYPE),
        move |ctx| {
            let sources = ctx
                .parent_value
                .try_downcast_ref::<T>()
                .map(|v| get(v).clone());
            FieldFuture::new(async move {
                Ok(Some(FieldValue::list(
                    sources?.into_iter().map(FieldValue::owned_any),
                )))
            })
        },
    )
}

/// 回答の型（`CmsAnswer` と、ストリーミングの最終判定 `CmsAnswerFinal`）。
fn answer_object(name: &str) -> Object {
    Object::new(name)
        .field(value_field::<Answer, _>(
            "status",
            TypeRef::named_nn(ANSWER_STATUS_TYPE),
            |a| Some(GqlValue::Enum(async_graphql::Name::new(a.status.name()))),
        ))
        .field(
            value_field::<Answer, _>("text", TypeRef::named_nn(TypeRef::STRING), |a| {
                Some(GqlValue::from(a.text.clone()))
            })
            .description("The answer in Markdown (the workspace's no-information phrase when NO_INFORMATION). Escape it before showing it."),
        )
        .field(
            value_field::<Answer, _>("truncated", TypeRef::named_nn(TypeRef::BOOLEAN), |a| {
                Some(GqlValue::from(a.truncated))
            })
            .description("Whether the text was cut at the length limit."),
        )
        .field(
            value_field::<Answer, _>("cached", TypeRef::named_nn(TypeRef::BOOLEAN), |a| {
                Some(GqlValue::from(a.cached))
            })
            .description("Whether the answer came from the answer cache (sources are checked against the current entries first)."),
        )
        .field(sources_field::<Answer>(|a| &a.sources))
}

fn source_type() -> Object {
    let text = |v: &Option<String>| Some(v.clone().map_or(GqlValue::Null, GqlValue::from));
    Object::new(ANSWER_SOURCE_TYPE)
        .description("A section of a published entry that the answer is based on.")
        .field(
            value_field::<Source, _>("citation", TypeRef::named_nn(TypeRef::INT), |s| {
                Some(GqlValue::from(i64::try_from(s.citation).unwrap_or(i64::MAX)))
            })
            .description(
                "The number of the citation markers such as [1] in the text that refer to this source.",
            ),
        )
        .field(value_field::<Source, _>(
            "entryId",
            TypeRef::named_nn(TypeRef::ID),
            |s| Some(GqlValue::from(s.entry_id.to_string())),
        ))
        .field(value_field::<Source, _>(
            "locale",
            TypeRef::named_nn(TypeRef::STRING),
            |s| Some(GqlValue::from(s.locale.clone())),
        ))
        .field(
            value_field::<Source, _>("url", TypeRef::named(TypeRef::STRING), move |s| {
                text(&s.url)
            })
            .description(
                "The entry's public URL, without the anchor. Link to `url#anchor` for the section.",
            ),
        )
        .field(value_field::<Source, _>(
            "anchor",
            TypeRef::named(TypeRef::STRING),
            move |s| text(&s.anchor),
        ))
        .field(value_field::<Source, _>(
            "breadcrumb",
            TypeRef::named_nn_list_nn(TypeRef::STRING),
            |s| {
                Some(GqlValue::List(
                    s.breadcrumb.iter().cloned().map(GqlValue::from).collect(),
                ))
            },
        ))
        .field(
            value_field::<Source, _>("versionId", TypeRef::named_nn(TypeRef::ID), |s| {
                Some(GqlValue::from(s.version_id.to_string()))
            })
            .description("The published version the answer was based on."),
        )
        .field(value_field::<Source, _>(
            "lastReviewedAt",
            TypeRef::named(TypeRef::STRING),
            move |s| text(&s.last_reviewed_at),
        ))
        .field(value_field::<Source, _>(
            "reviewExpired",
            TypeRef::named_nn(TypeRef::BOOLEAN),
            |s| Some(GqlValue::from(s.review_expired)),
        ))
}

fn status_type() -> Enum {
    [Status::Answered, Status::NoInformation, Status::ViolationDetected]
        .into_iter()
        .fold(Enum::new(ANSWER_STATUS_TYPE), |e, s| e.item(EnumItem::new(s.name())))
        .description("ANSWERED, NO_INFORMATION (nothing in the entries answers it), or VIOLATION_DETECTED (the streamed text broke the citation rule; show the final text).")
}

/// 回答APIの型（スキーマに登録する）。
pub fn types() -> (Vec<Object>, Enum, Union) {
    let verdict = Object::new(ANSWER_VERDICT_TYPE)
        .description("Sent first. When `answerable` is false, no text follows.")
        .field(value_field::<VerdictValue, _>(
            "answerable",
            TypeRef::named_nn(TypeRef::BOOLEAN),
            |v| Some(GqlValue::from(v.answerable)),
        ))
        .field(sources_field::<VerdictValue>(|v| &v.sources));
    let delta = Object::new(ANSWER_DELTA_TYPE)
        .description("A part of the answer text.")
        .field(value_field::<DeltaValue, _>(
            "text",
            TypeRef::named_nn(TypeRef::STRING),
            |d| Some(GqlValue::from(d.0.clone())),
        ));
    let event = Union::new(ANSWER_EVENT_TYPE)
        .possible_type(ANSWER_VERDICT_TYPE)
        .possible_type(ANSWER_DELTA_TYPE)
        .possible_type(ANSWER_FINAL_TYPE);
    (
        vec![
            answer_object(ANSWER_TYPE),
            answer_object(ANSWER_FINAL_TYPE),
            source_type(),
            verdict,
            delta,
        ],
        status_type(),
        event,
    )
}
