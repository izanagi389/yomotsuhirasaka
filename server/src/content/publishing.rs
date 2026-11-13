//! 公開・非公開・削除と版（03-content-and-editing.md「下書き・公開・版」、05-urls-and-redirects.md、06-corrections-and-freshness.md）。
//! ワークスペースのロールに切り替えたトランザクションの中で呼ぶ。
//!
//! - 公開は、下書きをそのまま版として固定する。送られたリビジョン番号が最新でなければ公開しない（見ていない変更を公開しないため）。
//! - 公開の前に、下書きをいまのスキーマでもう一度検査し、必須のフィールドと画像の代替テキスト（INV-IMG-01）を確かめる。
//! - 公開URLは公開時に計算して保存する。前に公開したときと変わったら、変更履歴に追記する（Q39, Q40、INV-URL-01）。
//!   コンテンツタイプにURL生成関数があればそれを組み込みJSで動かし、なければテンプレートから作る（Q38）。関数が失敗したら
//!   （例外、上限超え、正しくない値）、間違ったURLを公開して変更履歴に残さないよう、公開しない（原則8）。
//! - 公開のとき、本文のフィールドを変換器で Markdown と HTML にして版に保存する。変換に失敗したら（カスタムノードの
//!   変換関数の例外など）公開しない（間違った出力を公開しないため。URL生成関数と同じ）。
//! - 公開前のフック（beforePublish）が理由を返したら公開しない。フックが失敗したときも公開しない（URL生成関数と同じ）。
//! - 「訂正として公開」は、前の版から影響したセクションを求めて版に記録する（Q42、INV-COR-01）。前に一度も公開していない記事は訂正にできない。
//! - 版のメタデータ（番号、日時、URL、アンカーID、訂正の記録）は、本文を抹消しても残す（Q134）。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::entries::{self, Draft};
use super::{fields, sections};
use crate::schema::manifest::{ContentType, FieldType, Manifest};
use crate::script::{Engine, Failure, Host};

/// URL生成関数とフックを動かすための組（関数を動かすワークスペース）。
pub struct Scripts<'a> {
    pub engine: &'a Engine,
    pub host: Host,
}

const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;

/// テンプレートから公開URLを作る。スラッグは英小文字・数字・ハイフンだけ、ロケールはマニフェストの値なので、そのまま埋め込める。
pub fn public_url(template: &str, locale: &str, slug: &str) -> String {
    template.replace("{locale}", locale).replace("{slug}", slug)
}

/// 公開できなかった理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// 下書きが見つからない（削除済みを含む）。
    NotFound,
    /// リビジョン番号が最新でない。
    RevisionConflict,
    /// 下書きがいまのスキーマに合わない、または必須のフィールドが空。
    Invalid(Vec<String>),
    /// 代替テキストも「装飾画像」の印もない画像がある（場所の一覧）。
    AltTextMissing(Vec<String>),
    /// 公開中の別の記事が同じURLを使っている。
    UrlTaken,
    /// 一度も公開していない記事を訂正として公開しようとした。
    NothingToCorrect,
    /// URL生成関数が失敗した。
    UrlFunctionFailed(Failure),
    /// 公開前のフックが公開を止めた（フックの返した理由）。
    HookRejected(Vec<String>),
    /// 公開前のフックが失敗した。
    HookFailed(Failure),
    /// 本文を変換できなかった。
    ConversionFailed(Failure),
    /// 版の数の上限に達した（記事ごと、またはワークスペースごと）。
    TooManyVersions,
}

/// 版の数の上限（第三者の確認の R14）。公開のたびに全フィールドのコピーが増えるので、ディスクを埋めさせない。
/// 古い版は消さない（訂正の記録を後から取り返せなくしない。原則8）ので、上限に達したら公開を断る。
#[derive(Debug, Clone, Copy)]
pub struct VersionLimits {
    pub per_entry: u32,
    pub per_workspace: u32,
}

impl VersionLimits {
    pub fn from_config(limits: &crate::config::LimitsConfig) -> Self {
        Self {
            per_entry: limits.max_versions_per_entry,
            per_workspace: limits.max_versions_per_workspace,
        }
    }
}

/// 次の版（`number` 番目）を作れるか。
async fn room_for_version(
    conn: &mut PgConnection,
    limits: VersionLimits,
    number: i32,
) -> Result<bool> {
    if i64::from(number) > i64::from(limits.per_entry) {
        return Ok(false);
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM (SELECT 1 FROM versions LIMIT $1) t")
        .bind(i64::from(limits.per_workspace))
        .fetch_one(conn)
        .await?;
    Ok(count < i64::from(limits.per_workspace))
}

/// カスタムノードの変換関数（変換器に渡す形）。
pub fn custom_node_scripts(manifest: &Manifest) -> Vec<crate::script::CustomNodeScripts> {
    manifest
        .body
        .custom_nodes
        .iter()
        .filter_map(|c| {
            Some(crate::script::CustomNodeScripts {
                name: c.name.clone(),
                inline: c.inline,
                to_markdown: c.to_markdown.as_ref()?.source.clone(),
                to_html: c.to_html.as_ref().map(|s| s.source.clone()),
            })
        })
        .collect()
}

/// 本文のフィールドを変換する（フィールド名 → {markdown, html}）。
pub(crate) async fn render(
    scripts: &Scripts<'_>,
    manifest: &Manifest,
    ct: &ContentType,
    values: &Map<String, Value>,
) -> Result<Value, Failure> {
    let docs: Map<String, Value> = rich_text_fields(ct, values)
        .into_iter()
        .map(|(name, doc)| (name.to_owned(), doc.clone()))
        .collect();
    let out = scripts
        .engine
        .render(custom_node_scripts(manifest), &docs)
        .await?;
    serde_json::to_value(out).map_err(|_| Failure::Internal)
}

/// 同期のフックに渡す記事（`{id, contentType, locale, fields}`）。
pub fn hook_entry(draft: &Draft) -> Value {
    serde_json::json!({
        "id": draft.id,
        "contentType": draft.content_type,
        "locale": draft.locale,
        "fields": draft.fields,
    })
}

/// 公開前のフック。フックがなければ理由なし。
async fn before_publish(
    scripts: &Scripts<'_>,
    ct: &ContentType,
    draft: &Draft,
) -> Result<Vec<String>, Failure> {
    let Some(hook) = &ct.hooks.before_publish else {
        return Ok(Vec::new());
    };
    let input = serde_json::json!({ "entry": hook_entry(draft) });
    scripts
        .engine
        .before_publish(hook, scripts.host.clone(), &input)
        .await
}

/// 公開の結果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Published {
    pub version_id: Uuid,
    pub number: i32,
    pub url: String,
    pub is_correction: bool,
    /// URLが変わったときの変更前のURL。
    pub previous_url: Option<String>,
    /// この公開の前から公開中だったか（Webhookのイベントの種類を決める。応答には含めない）。
    #[serde(skip)]
    pub was_published: bool,
}

/// 記事の行をロックして読む（削除済みは None）。(リビジョン番号, 公開中の版, 前に公開したURL)
async fn lock(
    conn: &mut PgConnection,
    entry_id: Uuid,
) -> Result<Option<(i64, Option<Uuid>, Option<String>)>> {
    Ok(sqlx::query_as(
        "SELECT draft_revision, published_version_id, url FROM entries
         WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(entry_id)
    .fetch_optional(conn)
    .await?)
}

fn rich_texts<'a>(ct: &'a ContentType, values: &'a Map<String, Value>) -> Vec<&'a Value> {
    rich_text_fields(ct, values)
        .into_iter()
        .map(|(_, v)| v)
        .collect()
}

/// 下書きをいまのスキーマで検査した問題（スキーマに合わない、または公開に必須のフィールドが空）。
fn problems(manifest: &Manifest, ct: &ContentType, draft: &Draft) -> Vec<String> {
    if let Err(problems) = fields::convert_all(manifest, ct, &draft.fields) {
        return problems;
    }
    let mut problems = Vec::new();
    for f in &ct.fields {
        let value = draft.fields.get(&f.name);
        let empty = match (f.kind, value) {
            (FieldType::Richtext, Some(v)) if v.is_object() => {
                sections::is_blank(v, &manifest.body)
            }
            _ => fields::is_empty(value),
        };
        if empty && (f.required || f.kind == FieldType::Slug) {
            problems.push(format!("fields.{}: required to publish", f.name));
        }
    }
    problems
}

/// 代替テキストも「装飾画像」の印もない画像の場所（`fields.<名前>.content[..]`、画像のフィールドは `fields.<名前>`）。
fn missing_alt(ct: &ContentType, draft: &Draft) -> Vec<String> {
    let mut missing = Vec::new();
    for f in &ct.fields {
        let Some(value) = draft.fields.get(&f.name).filter(|v| v.is_object()) else {
            continue;
        };
        match f.kind {
            FieldType::Richtext => missing.extend(
                sections::images_missing_alt(value)
                    .into_iter()
                    .map(|p| format!("fields.{}{}", f.name, p.trim_start_matches('$'))),
            ),
            // 画像のフィールドも、本文の画像と同じ規則（INV-IMG-01）
            FieldType::Image if !sections::has_alt(value) => {
                missing.push(format!("fields.{}", f.name))
            }
            _ => {}
        }
    }
    missing
}

/// 下書きが公開できる状態か確かめる（いまのスキーマでの検査、必須、代替テキスト）。
fn check(manifest: &Manifest, ct: &ContentType, draft: &Draft) -> Result<(), Refused> {
    let problems = problems(manifest, ct, draft);
    if !problems.is_empty() {
        return Err(Refused::Invalid(problems));
    }
    let missing = missing_alt(ct, draft);
    if !missing.is_empty() {
        return Err(Refused::AltTextMissing(missing));
    }
    Ok(())
}

/// 公開URL（スラッグのフィールドが空なら None）。
async fn url_of(
    scripts: &Scripts<'_>,
    ct: &ContentType,
    draft: &Draft,
) -> Result<Option<String>, Failure> {
    Ok(
        compute_url(scripts, ct, draft.id, &draft.locale, &draft.fields)
            .await?
            .map(|(url, _)| url),
    )
}

/// 記事のフィールドから公開URLを作る（スラッグのフィールドが空なら None）。
/// URL生成関数を使ったときは、関数がホスト関数で読もうとした記事のID（依存先）も返す（Q39）。
pub async fn compute_url(
    scripts: &Scripts<'_>,
    ct: &ContentType,
    id: Uuid,
    locale: &str,
    fields: &Map<String, Value>,
) -> Result<Option<(String, Vec<Uuid>)>, Failure> {
    let Some(slug) = ct
        .fields
        .iter()
        .find(|f| f.kind == FieldType::Slug)
        .and_then(|f| fields.get(&f.name))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    let Some(function) = &ct.url_function else {
        return Ok(Some((
            public_url(&ct.url_template, locale, slug),
            Vec::new(),
        )));
    };
    let input = serde_json::json!({
        "id": id,
        "contentType": ct.name,
        "locale": locale,
        "slug": slug,
        "fields": fields,
    });
    let host = scripts.host.tracking();
    let url = scripts.engine.url(function, host.clone(), &input).await?;
    Ok(Some((url, host.reads())))
}

/// いちばん新しい版の番号と、本文（抹消していれば None）。
pub(crate) async fn latest_version(
    conn: &mut PgConnection,
    entry_id: Uuid,
) -> Result<Option<(i32, Option<Value>)>> {
    let latest: Option<(i32, Option<String>)> = sqlx::query_as(
        "SELECT number, fields::text FROM versions WHERE entry_id = $1 ORDER BY number DESC LIMIT 1",
    )
    .bind(entry_id)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(match latest {
        Some((n, Some(text))) => Some((n, Some(serde_json::from_str(&text)?))),
        Some((n, None)) => Some((n, None)),
        None => None,
    })
}

async fn url_taken(conn: &mut PgConnection, url: &str, entry_id: Uuid) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM entries WHERE url = $1 AND published_version_id IS NOT NULL AND id <> $2)",
    )
    .bind(url)
    .bind(entry_id)
    .fetch_one(conn)
    .await?)
}

/// 本文のフィールドの名前と値（フィールドの順。値が本文でないものは除く）。
pub(crate) fn rich_text_fields<'a>(
    ct: &'a ContentType,
    values: &'a Map<String, Value>,
) -> Vec<(&'a str, &'a Value)> {
    ct.fields
        .iter()
        .filter(|f| f.kind == FieldType::Richtext)
        .filter_map(|f| values.get(&f.name).map(|v| (f.name.as_str(), v)))
        .filter(|(_, v)| v.is_object())
        .collect()
}

/// 公開の確認画面の1行（変わったセクション）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckedSection {
    /// 導入のときの本文のフィールドの名前。
    pub field: Option<String>,
    #[serde(flatten)]
    pub change: sections::SectionChange,
}

/// 公開の確認画面に出す情報（何も書き換えない。Q68, Q75）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishCheck {
    /// 検査した下書きのリビジョン番号（公開のときにそのまま送る）。
    pub revision: i64,
    /// いま公開中か。
    pub published: bool,
    /// 訂正として公開できるか（前に一度でも公開したか）。
    pub can_correct: bool,
    /// 公開を止める問題（スキーマに合わない、必須のフィールドが空）。
    pub problems: Vec<String>,
    /// 代替テキストも「装飾画像」の印もない画像の場所。
    pub images_missing_alt: Vec<String>,
    /// 公開したときのURL（スラッグが空なら None）。
    pub url: Option<String>,
    /// URLが変わるときの、いまのURL。
    pub previous_url: Option<String>,
    /// 公開中の別の記事が同じURLを使っているか。
    pub url_taken: bool,
    /// 比べた版の番号（いちばん新しい版）。
    pub compared_with: Option<i32>,
    /// 前の版から変わったセクション。前の版がない、または本文を抹消していれば None。
    pub sections: Option<Vec<CheckedSection>>,
    /// 原文に未確認の更新があれば、その情報（公開は止めない。Q162, Q164）。
    pub source_update: Option<super::translations::SourceUpdate>,
}

/// 公開の前の確認（公開と同じ判定を、書き込まずに行う）。記事がなければ None。
pub async fn check_before_publish(
    conn: &mut PgConnection,
    manifest: &Manifest,
    scripts: &Scripts<'_>,
    entry_id: Uuid,
) -> Result<Option<PublishCheck>> {
    let row: Option<(i64, Option<Uuid>, Option<String>)> = sqlx::query_as(
        "SELECT draft_revision, published_version_id, url FROM entries WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(entry_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((revision, current, previous_url)) = row else {
        return Ok(None);
    };
    let Some(draft) = entries::draft(conn, manifest, entry_id).await? else {
        return Ok(None);
    };
    let Some(ct) = manifest.content_type(&draft.content_type) else {
        return Ok(None);
    };
    let latest = latest_version(conn, entry_id).await?;
    let mut problems = problems(manifest, ct, &draft);
    match before_publish(scripts, ct, &draft).await {
        Ok(reasons) => problems.extend(reasons.into_iter().map(|r| format!("hook: {r}"))),
        Err(f) => problems.push(format!(
            "hook: the beforePublish hook failed ({})",
            f.describe()
        )),
    }
    if let Err(f) = render(scripts, manifest, ct, &draft.fields).await {
        problems.push(format!("body: cannot convert the body ({})", f.describe()));
    }
    let url = url_of(scripts, ct, &draft).await.unwrap_or_else(|f| {
        problems.push(format!(
            "url: cannot make the public URL ({})",
            f.describe()
        ));
        None
    });
    let url_taken = match &url {
        Some(u) => url_taken(conn, u, entry_id).await?,
        None => false,
    };
    let sections = match &latest {
        Some((_, Some(Value::Object(before)))) => {
            let before = rich_text_fields(ct, before);
            let after = rich_text_fields(ct, &draft.fields);
            let before_docs: Vec<&Value> = before.iter().map(|(_, v)| *v).collect();
            let after_docs: Vec<&Value> = after.iter().map(|(_, v)| *v).collect();
            Some(
                sections::compare(&before_docs, &after_docs)
                    .into_iter()
                    .map(|change| CheckedSection {
                        field: change.lead_of.and_then(|i| {
                            after.get(i).or(before.get(i)).map(|(n, _)| (*n).to_owned())
                        }),
                        change,
                    })
                    .collect(),
            )
        }
        _ => None,
    };
    Ok(Some(PublishCheck {
        revision,
        published: current.is_some(),
        can_correct: latest.is_some(),
        problems,
        images_missing_alt: missing_alt(ct, &draft),
        previous_url: previous_url.filter(|p| url.as_ref().is_some_and(|u| u != p)),
        url,
        url_taken,
        compared_with: latest.as_ref().map(|(n, _)| *n),
        sections,
        source_update: super::translations::source_update_of(conn, entry_id).await?,
    }))
}

/// 公開の準備（検査と計算）が済んだ下書き。その場の公開と、予約公開（Q78。予約のときに作って保存する）で使う。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prepared {
    /// 公開するフィールドのスナップショット。
    pub fields: Map<String, Value>,
    pub url: String,
    /// URL生成関数が読んだ記事。
    pub dependencies: Vec<Uuid>,
    /// 本文の変換結果。
    pub rendered: Value,
    /// 本文のアンカーID。
    pub anchors: Vec<String>,
}

/// 下書きを検査し、公開に要るものを計算する（スキーマの検査、公開前のフック、URL生成関数、URLの重なり、本文の変換）。
async fn prepare(
    conn: &mut PgConnection,
    manifest: &Manifest,
    scripts: &Scripts<'_>,
    entry_id: Uuid,
    draft: &Draft,
) -> Result<Result<Prepared, Refused>> {
    let Some(ct) = manifest.content_type(&draft.content_type) else {
        return Ok(Err(Refused::NotFound));
    };
    if let Err(r) = check(manifest, ct, draft) {
        return Ok(Err(r));
    }
    match before_publish(scripts, ct, draft).await {
        Ok(reasons) if reasons.is_empty() => {}
        Ok(reasons) => return Ok(Err(Refused::HookRejected(reasons))),
        Err(f) => return Ok(Err(Refused::HookFailed(f))),
    }
    // スラッグは公開に必須なので、check を通っていれば空でない
    let (url, dependencies) =
        match compute_url(scripts, ct, draft.id, &draft.locale, &draft.fields).await {
            Ok(url) => url.unwrap_or_default(),
            Err(f) => return Ok(Err(Refused::UrlFunctionFailed(f))),
        };
    if url_taken(conn, &url, entry_id).await? {
        return Ok(Err(Refused::UrlTaken));
    }
    let rendered = match render(scripts, manifest, ct, &draft.fields).await {
        Ok(r) => r,
        Err(f) => return Ok(Err(Refused::ConversionFailed(f))),
    };
    let anchors: Vec<String> = rich_texts(ct, &draft.fields)
        .into_iter()
        .flat_map(sections::anchors)
        .collect();
    Ok(Ok(Prepared {
        fields: draft.fields.clone(),
        url,
        dependencies,
        rendered,
        anchors,
    }))
}

/// 下書きを公開する。
#[allow(clippy::too_many_arguments)]
pub async fn publish(
    conn: &mut PgConnection,
    manifest: &Manifest,
    scripts: &Scripts<'_>,
    schema_version: i32,
    entry_id: Uuid,
    expected_revision: i64,
    correction: bool,
    published_by: Uuid,
    limits: VersionLimits,
) -> Result<Result<Published, Refused>> {
    let Some((revision, current, previous_url)) = lock(conn, entry_id).await? else {
        return Ok(Err(Refused::NotFound));
    };
    if revision != expected_revision {
        return Ok(Err(Refused::RevisionConflict));
    }
    let latest = latest_version(conn, entry_id).await?;
    if correction && latest.is_none() {
        return Ok(Err(Refused::NothingToCorrect));
    }
    let Some(draft) = entries::draft(conn, manifest, entry_id).await? else {
        return Ok(Err(Refused::NotFound));
    };
    let Some(ct) = manifest.content_type(&draft.content_type) else {
        return Ok(Err(Refused::NotFound));
    };
    let prepared = match prepare(conn, manifest, scripts, entry_id, &draft).await? {
        Ok(p) => p,
        Err(r) => return Ok(Err(r)),
    };
    // 訂正: 前の版と比べて影響したセクション。前の版の本文が抹消されていれば求められない（NULL）
    let affected = match (&latest, correction) {
        (Some((_, Some(Value::Object(before)))), true) => {
            let a = sections::affected(&rich_texts(ct, before), &rich_texts(ct, &draft.fields));
            Some((a.anchors.into_iter().collect::<Vec<_>>(), a.lead))
        }
        _ => None,
    };
    let number = latest.as_ref().map_or(1, |(n, _)| n + 1);
    if !room_for_version(conn, limits, number).await? {
        return Ok(Err(Refused::TooManyVersions));
    }
    insert_version(
        conn,
        entry_id,
        number,
        schema_version,
        &prepared,
        correction,
        affected,
        published_by,
        current.is_some(),
        previous_url,
    )
    .await
    .map(Ok)
}

/// 予約公開の準備（Q78）: 下書きのリビジョンを確かめ、公開と同じ検査と計算を済ませる。訂正は予約できない（呼び出し側で拒む）。
pub async fn prepare_scheduled(
    conn: &mut PgConnection,
    manifest: &Manifest,
    scripts: &Scripts<'_>,
    entry_id: Uuid,
    expected_revision: i64,
) -> Result<Result<Prepared, Refused>> {
    let Some((revision, _, _)) = lock(conn, entry_id).await? else {
        return Ok(Err(Refused::NotFound));
    };
    if revision != expected_revision {
        return Ok(Err(Refused::RevisionConflict));
    }
    let Some(draft) = entries::draft(conn, manifest, entry_id).await? else {
        return Ok(Err(Refused::NotFound));
    };
    prepare(conn, manifest, scripts, entry_id, &draft).await
}

/// 準備済みの予約版を公開する（予約の時刻に）。公開日時は実際に公開した日時。URLの重なりはこの時点でも確かめる。
pub async fn publish_prepared(
    conn: &mut PgConnection,
    schema_version: i32,
    entry_id: Uuid,
    prepared: &Prepared,
    published_by: Uuid,
    limits: VersionLimits,
) -> Result<Result<Published, Refused>> {
    let Some((_, current, previous_url)) = lock(conn, entry_id).await? else {
        return Ok(Err(Refused::NotFound));
    };
    if url_taken(conn, &prepared.url, entry_id).await? {
        return Ok(Err(Refused::UrlTaken));
    }
    let number = latest_version(conn, entry_id)
        .await?
        .map_or(1, |(n, _)| n + 1);
    if !room_for_version(conn, limits, number).await? {
        return Ok(Err(Refused::TooManyVersions));
    }
    insert_version(
        conn,
        entry_id,
        number,
        schema_version,
        prepared,
        false,
        None,
        published_by,
        current.is_some(),
        previous_url,
    )
    .await
    .map(Ok)
}

/// 版を作り、記事の公開中の版とURLを切り替え、URLの依存と変更履歴を記録する。
#[allow(clippy::too_many_arguments)]
async fn insert_version(
    conn: &mut PgConnection,
    entry_id: Uuid,
    number: i32,
    schema_version: i32,
    prepared: &Prepared,
    correction: bool,
    affected: Option<(Vec<String>, bool)>,
    published_by: Uuid,
    was_published: bool,
    previous_url: Option<String>,
) -> Result<Published> {
    let url = prepared.url.clone();
    let (affected_anchors, affects_lead) = match affected {
        Some((a, lead)) => (Some(a), Some(lead)),
        None => (None, None),
    };
    let version_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO versions (id, entry_id, number, schema_version, fields, published_by, correction,
                               url, anchors, affected_anchors, affects_lead, rendered, rendered_with)
         VALUES ($1, $2, $3, $4, $5::jsonb, $6, $7, $8, $9, $10, $11, $12::jsonb, $13)",
    )
    .bind(version_id)
    .bind(entry_id)
    .bind(number)
    .bind(schema_version)
    .bind(Value::Object(prepared.fields.clone()).to_string())
    .bind(published_by)
    .bind(correction)
    .bind(&url)
    .bind(&prepared.anchors)
    .bind(affected_anchors.as_deref())
    .bind(affects_lead)
    .bind(prepared.rendered.to_string())
    .bind(crate::script::CONVERTER_VERSION)
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE entries SET published_version_id = $2, url = $3 WHERE id = $1")
        .bind(entry_id)
        .bind(version_id)
        .bind(&url)
        .execute(&mut *conn)
        .await?;
    super::urls::record_dependencies(conn, entry_id, &prepared.dependencies).await?;
    let previous_url = previous_url.filter(|p| *p != url);
    if let Some(from) = &previous_url {
        sqlx::query(
            "INSERT INTO url_changes (id, entry_id, from_url, to_url, version_id) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(Uuid::new_v4())
        .bind(entry_id)
        .bind(from)
        .bind(&url)
        .bind(version_id)
        .execute(&mut *conn)
        .await?;
    }
    Ok(Published {
        version_id,
        number,
        url,
        is_correction: correction,
        previous_url,
        was_published,
    })
}

/// 非公開にする。記事がなければ None、公開中だったかを返す。
pub async fn unpublish(conn: &mut PgConnection, entry_id: Uuid) -> Result<Option<bool>> {
    let Some((_, current, _)) = lock(conn, entry_id).await? else {
        return Ok(None);
    };
    if current.is_some() {
        sqlx::query("UPDATE entries SET published_version_id = NULL WHERE id = $1")
            .bind(entry_id)
            .execute(&mut *conn)
            .await?;
    }
    Ok(Some(current.is_some()))
}

/// 記事を削除する（公開も止める）。記事がなければ false。
pub async fn delete(conn: &mut PgConnection, entry_id: Uuid) -> Result<bool> {
    if lock(conn, entry_id).await?.is_none() {
        return Ok(false);
    }
    sqlx::query("UPDATE entries SET published_version_id = NULL, deleted_at = now() WHERE id = $1")
        .bind(entry_id)
        .execute(&mut *conn)
        .await?;
    Ok(true)
}

/// 記事が（削除されずに）あるか。
pub async fn exists(conn: &mut PgConnection, entry_id: Uuid) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM entries WHERE id = $1 AND deleted_at IS NULL)",
    )
    .bind(entry_id)
    .fetch_one(conn)
    .await?)
}

/// 版のメタデータ。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub id: Uuid,
    pub number: i32,
    pub schema_version: i32,
    pub published_by: Option<Uuid>,
    pub published_at: String,
    pub url: Option<String>,
    /// 公開中の版か。
    pub current: bool,
    pub is_correction: bool,
    /// 訂正で影響したセクションのアンカーID（訂正でない、または求められなかったときは null）。
    pub affected_anchors: Option<Vec<String>>,
    pub affects_lead: Option<bool>,
    pub anchors: Vec<String>,
    pub erased: bool,
    pub erased_at: Option<String>,
    /// 訂正の説明（訂正として公開した版で、書いたときだけ。管理画面だけ）。
    pub correction_note: Option<String>,
}

/// 版の中身（抹消した版の fields は null）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionDetail {
    #[serde(flatten)]
    pub info: VersionInfo,
    pub fields: Option<Value>,
}

type VersionRow = (
    Uuid,
    i32,
    i32,
    Option<Uuid>,
    String,
    Option<String>,
    bool,
    bool,
    Option<Vec<String>>,
    Option<bool>,
    Vec<String>,
    Option<String>,
    Option<String>,
);

const VERSION_COLUMNS: &str = r#"v.id, v.number, v.schema_version, v.published_by,
    to_char(v.published_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'), v.url,
    COALESCE(v.id = e.published_version_id, false), v.correction, v.affected_anchors, v.affects_lead, v.anchors,
    to_char(v.erased_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'),
    (SELECT n.text FROM correction_notes n WHERE n.version_id = v.id)"#;

fn info(r: VersionRow) -> VersionInfo {
    let (
        id,
        number,
        schema_version,
        published_by,
        published_at,
        url,
        current,
        is_correction,
        affected_anchors,
        affects_lead,
        anchors,
        erased_at,
        correction_note,
    ) = r;
    VersionInfo {
        id,
        number,
        schema_version,
        published_by,
        published_at,
        url,
        current,
        is_correction,
        affected_anchors,
        affects_lead,
        anchors,
        erased: erased_at.is_some(),
        erased_at,
        correction_note,
    }
}

/// 版の一覧（古い順）。記事がなければ None。
pub async fn versions(conn: &mut PgConnection, entry_id: Uuid) -> Result<Option<Vec<VersionInfo>>> {
    if !exists(conn, entry_id).await? {
        return Ok(None);
    }
    let rows: Vec<VersionRow> = sqlx::query_as(crate::sql!(
        "SELECT {VERSION_COLUMNS} FROM versions v JOIN entries e ON e.id = v.entry_id
         WHERE v.entry_id = $1 ORDER BY v.number"
    ))
    .bind(entry_id)
    .fetch_all(conn)
    .await?;
    Ok(Some(rows.into_iter().map(info).collect()))
}

/// 版を1つ読む。抹消した版は本文を返さない（INV-PUB-04）。
pub async fn version(
    conn: &mut PgConnection,
    entry_id: Uuid,
    version_id: Uuid,
) -> Result<Option<VersionDetail>> {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT CASE WHEN v.erased_at IS NULL THEN v.fields::text END
         FROM versions v JOIN entries e ON e.id = v.entry_id
         WHERE v.entry_id = $1 AND v.id = $2 AND e.deleted_at IS NULL",
    )
    .bind(entry_id)
    .bind(version_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((fields,)) = row else {
        return Ok(None);
    };
    let fields = fields.map(|t| serde_json::from_str(&t)).transpose()?;
    let r: VersionRow = sqlx::query_as(crate::sql!(
        "SELECT {VERSION_COLUMNS} FROM versions v JOIN entries e ON e.id = v.entry_id WHERE v.id = $1"
    ))
    .bind(version_id)
    .fetch_one(conn)
    .await?;
    Ok(Some(VersionDetail {
        info: info(r),
        fields,
    }))
}

/// 公開URLの変更。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UrlChange {
    pub from: String,
    pub to: String,
    pub version_id: Option<Uuid>,
    pub changed_at: String,
}

/// 公開URLの変更履歴（古い順）。記事がなければ None。
pub async fn url_history(
    conn: &mut PgConnection,
    entry_id: Uuid,
) -> Result<Option<Vec<UrlChange>>> {
    if !exists(conn, entry_id).await? {
        return Ok(None);
    }
    let rows: Vec<(String, String, Option<Uuid>, String)> = sqlx::query_as(crate::sql!(
        "SELECT from_url, to_url, version_id, to_char(changed_at AT TIME ZONE 'UTC', {RFC3339})
         FROM url_changes WHERE entry_id = $1 ORDER BY changed_at, id"
    ))
    .bind(entry_id)
    .fetch_all(conn)
    .await?;
    Ok(Some(
        rows.into_iter()
            .map(|(from, to, version_id, changed_at)| UrlChange {
                from,
                to,
                version_id,
                changed_at,
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_from_templates() {
        assert_eq!(
            public_url("https://example.com/{locale}/blog/{slug}", "ja", "hello-1"),
            "https://example.com/ja/blog/hello-1"
        );
        assert_eq!(
            public_url("https://example.com/{slug}", "en", "a"),
            "https://example.com/a"
        );
    }
}
