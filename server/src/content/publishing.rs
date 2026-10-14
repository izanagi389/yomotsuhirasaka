//! 公開・非公開・削除と版（03-content-and-editing.md「下書き・公開・版」、05-urls-and-redirects.md、06-corrections-and-freshness.md）。
//! ワークスペースのロールに切り替えたトランザクションの中で呼ぶ。
//!
//! - 公開は、下書きをそのまま版として固定する。送られたリビジョン番号が最新でなければ公開しない（見ていない変更を公開しないため）。
//! - 公開の前に、下書きをいまのスキーマでもう一度検査し、必須のフィールドと画像の代替テキスト（INV-IMG-01）を確かめる。
//! - 公開URLはテンプレートから公開時に計算して保存する。前に公開したときと変わったら、変更履歴に追記する（Q39, Q40、INV-URL-01）。
//! - 「訂正として公開」は、前の版から影響したセクションを求めて版に記録する（Q42、INV-COR-01）。前に一度も公開していない記事は訂正にできない。
//! - 版のメタデータ（番号、日時、URL、アンカーID、訂正の記録）は、本文を抹消しても残す（Q134）。

use anyhow::Result;
use serde::Serialize;
use serde_json::{Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::entries::{self, Draft};
use super::{fields, sections};
use crate::schema::manifest::{ContentType, FieldType, Manifest};

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

fn rich_texts<'a>(ct: &ContentType, values: &'a Map<String, Value>) -> Vec<&'a Value> {
    ct.fields
        .iter()
        .filter(|f| f.kind == FieldType::Richtext)
        .filter_map(|f| values.get(&f.name))
        .filter(|v| v.is_object())
        .collect()
}

/// 下書きが公開できる状態か確かめる（いまのスキーマでの検査、必須、代替テキスト）。
fn check(manifest: &Manifest, ct: &ContentType, draft: &Draft) -> Result<(), Refused> {
    if let Err(problems) = fields::convert_all(manifest, ct, &draft.fields) {
        return Err(Refused::Invalid(problems));
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
    if !problems.is_empty() {
        return Err(Refused::Invalid(problems));
    }
    let mut missing = Vec::new();
    for f in ct.fields.iter().filter(|f| f.kind == FieldType::Richtext) {
        if let Some(doc) = draft.fields.get(&f.name).filter(|v| v.is_object()) {
            missing.extend(
                sections::images_missing_alt(doc)
                    .into_iter()
                    .map(|p| format!("fields.{}{}", f.name, p.trim_start_matches('$'))),
            );
        }
    }
    if !missing.is_empty() {
        return Err(Refused::AltTextMissing(missing));
    }
    Ok(())
}

/// 下書きを公開する。
#[allow(clippy::too_many_arguments)]
pub async fn publish(
    conn: &mut PgConnection,
    manifest: &Manifest,
    schema_version: i32,
    entry_id: Uuid,
    expected_revision: i64,
    correction: bool,
    published_by: Uuid,
) -> Result<Result<Published, Refused>> {
    let Some((revision, current, previous_url)) = lock(conn, entry_id).await? else {
        return Ok(Err(Refused::NotFound));
    };
    if revision != expected_revision {
        return Ok(Err(Refused::RevisionConflict));
    }
    let latest: Option<(i32, Option<String>)> = sqlx::query_as(
        "SELECT number, fields::text FROM versions WHERE entry_id = $1 ORDER BY number DESC LIMIT 1",
    )
    .bind(entry_id)
    .fetch_optional(&mut *conn)
    .await?;
    let latest: Option<(i32, Option<Value>)> = match latest {
        Some((n, Some(text))) => Some((n, Some(serde_json::from_str(&text)?))),
        Some((n, None)) => Some((n, None)),
        None => None,
    };
    if correction && latest.is_none() {
        return Ok(Err(Refused::NothingToCorrect));
    }
    let Some(draft) = entries::draft(conn, manifest, entry_id).await? else {
        return Ok(Err(Refused::NotFound));
    };
    let Some(ct) = manifest.content_type(&draft.content_type) else {
        return Ok(Err(Refused::NotFound));
    };
    if let Err(r) = check(manifest, ct, &draft) {
        return Ok(Err(r));
    }
    let slug = ct
        .fields
        .iter()
        .find(|f| f.kind == FieldType::Slug)
        .and_then(|f| draft.fields.get(&f.name))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let url = public_url(&ct.url_template, &draft.locale, slug);
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM entries WHERE url = $1 AND published_version_id IS NOT NULL AND id <> $2)",
    )
    .bind(&url)
    .bind(entry_id)
    .fetch_one(&mut *conn)
    .await?;
    if taken {
        return Ok(Err(Refused::UrlTaken));
    }

    let snapshot = Value::Object(draft.fields.clone());
    let anchors: Vec<String> = rich_texts(ct, &draft.fields)
        .into_iter()
        .flat_map(sections::anchors)
        .collect();
    // 訂正: 前の版と比べて影響したセクション。前の版の本文が抹消されていれば求められない（NULL）
    let (affected_anchors, affects_lead) = match (&latest, correction) {
        (Some((_, Some(Value::Object(before)))), true) => {
            let a = sections::affected(&rich_texts(ct, before), &rich_texts(ct, &draft.fields));
            (
                Some(a.anchors.into_iter().collect::<Vec<_>>()),
                Some(a.lead),
            )
        }
        _ => (None, None),
    };
    let number = latest.as_ref().map_or(1, |(n, _)| n + 1);
    let version_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO versions (id, entry_id, number, schema_version, fields, published_by, correction,
                               url, anchors, affected_anchors, affects_lead)
         VALUES ($1, $2, $3, $4, $5::jsonb, $6, $7, $8, $9, $10, $11)",
    )
    .bind(version_id)
    .bind(entry_id)
    .bind(number)
    .bind(schema_version)
    .bind(snapshot.to_string())
    .bind(published_by)
    .bind(correction)
    .bind(&url)
    .bind(&anchors)
    .bind(affected_anchors.as_deref())
    .bind(affects_lead)
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE entries SET published_version_id = $2, url = $3 WHERE id = $1")
        .bind(entry_id)
        .bind(version_id)
        .bind(&url)
        .execute(&mut *conn)
        .await?;
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
    Ok(Ok(Published {
        version_id,
        number,
        url,
        is_correction: correction,
        previous_url,
        was_published: current.is_some(),
    }))
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
);

const VERSION_COLUMNS: &str = r#"v.id, v.number, v.schema_version, v.published_by,
    to_char(v.published_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'), v.url,
    COALESCE(v.id = e.published_version_id, false), v.correction, v.affected_anchors, v.affects_lead, v.anchors,
    to_char(v.erased_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"')"#;

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
