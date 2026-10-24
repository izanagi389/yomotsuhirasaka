//! 翻訳の作成と、元の記事・版の記録（03-content-and-editing.md、Q154, Q156, Q159）。
//! ワークスペースのロールに切り替えたトランザクションの中で呼ぶ。
//!
//! - 新しいロケールの記事は、元の記事の下書きを複製して作り、同じ翻訳グループに入れる。
//! - どの記事を元にしたかと、その時点の元の記事の公開中の版を「反映済み」として記録する（後から取り返せない記録、Q159）。
//! - 元の記事が削除されていれば、読むときに「元なし」とする（Q156）。記録そのものは消さない。
//! - 1つの翻訳グループの中で、1つのロケールの記事は1つだけ。
//! - 原文の更新の知らせ（M2、Q155, Q159, Q162）: 元の記事のいちばん新しい版が、反映済みの版と違えば「原文が更新された」とする。
//!   保存した印は使わず、読むたびに判定する。元の記事が非公開でも、版が進んでいれば更新ありとする。
//! - 反映済みの版は「原文の更新を反映済み」の操作（`mark_reflected`）と、元の記事の設定し直し（`set_source`、Q156）でだけ進む。
//!   翻訳の記事を公開しても進まない。

use anyhow::Result;
use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

use serde_json::{Map, Value};

use super::entries::{self, Draft};
use super::fields::{self, Column};
use super::publishing::rich_text_fields;
use super::sections::{self, ChangeKind};
use crate::schema::manifest::{FieldType, Manifest};

/// 翻訳の情報。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationInfo {
    pub group_id: Uuid,
    pub locale: String,
    /// 元の記事（元なしなら null）。
    pub source_entry_id: Option<Uuid>,
    pub source_locale: Option<String>,
    /// 反映済みとして記録した元の記事の版（元なし、または作成の時点で元の記事が公開されていなかったら null）。
    pub reflected_source_version_id: Option<Uuid>,
    /// 原文の更新（反映済みの版より後に元の記事の版がある）。なければ null。
    pub source_update: Option<SourceUpdate>,
    /// 同じ翻訳グループの記事（この記事を含む）。
    pub members: Vec<Member>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub entry_id: Uuid,
    pub locale: String,
    pub published: bool,
}

const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;

/// 原文の更新。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceUpdate {
    /// 元の記事のいちばん新しい版。
    pub latest_version_id: Uuid,
    /// 反映済みの版より後に「訂正として公開」した版がある（「原文が訂正されました」。06、Q155）。
    pub corrected: bool,
}

/// 元の記事のいちばん新しい版（版がなければ None）。
async fn latest_version_of(conn: &mut PgConnection, entry_id: Uuid) -> Result<Option<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM versions WHERE entry_id = $1 ORDER BY number DESC LIMIT 1",
    )
    .bind(entry_id)
    .fetch_optional(conn)
    .await?)
}

/// 原文の更新があるか（元の記事と、反映済みの版から判定する）。
async fn source_update(
    conn: &mut PgConnection,
    source_id: Uuid,
    reflected: Option<Uuid>,
) -> Result<Option<SourceUpdate>> {
    let Some(latest) = latest_version_of(conn, source_id).await? else {
        return Ok(None);
    };
    if reflected == Some(latest) {
        return Ok(None);
    }
    // 反映済みの版がない（または元の記事の版でない）ときは、すべての版を「後の版」とみなす
    let corrected: bool = sqlx::query_scalar(
        "SELECT EXISTS (
           SELECT 1 FROM versions v
           WHERE v.entry_id = $1 AND v.correction
             AND v.number > COALESCE((SELECT number FROM versions WHERE id = $2 AND entry_id = $1), 0))",
    )
    .bind(source_id)
    .bind(reflected)
    .fetch_one(conn)
    .await?;
    Ok(Some(SourceUpdate {
        latest_version_id: latest,
        corrected,
    }))
}

/// 原文の更新がある記事（「やること」の一覧。元の記事の今の版が古い順）。
#[derive(Debug, Clone)]
pub struct Updated {
    pub entry_id: Uuid,
    pub corrected: bool,
    /// 元の記事のいちばん新しい版を公開した時刻。
    pub published_at: String,
}

/// ワークスペースの、原文の更新がある記事（判定は `source_update` と同じ）。
pub async fn updated(conn: &mut PgConnection) -> Result<Vec<Updated>> {
    let rows: Vec<(Uuid, bool, String)> = sqlx::query_as(crate::sql!(
        "SELECT e.id,
                EXISTS (
                  SELECT 1 FROM versions v
                  WHERE v.entry_id = s.id AND v.correction
                    AND v.number > COALESCE((SELECT number FROM versions WHERE id = e.reflected_source_version_id AND entry_id = s.id), 0)),
                to_char(lv.published_at AT TIME ZONE 'UTC', {RFC3339})
         FROM entries e
         JOIN entries s ON s.id = e.source_entry_id AND s.deleted_at IS NULL
         JOIN LATERAL (SELECT id, published_at FROM versions WHERE entry_id = s.id ORDER BY number DESC LIMIT 1) lv ON true
         WHERE e.deleted_at IS NULL AND lv.id IS DISTINCT FROM e.reflected_source_version_id
         ORDER BY lv.published_at, e.id
         LIMIT 500"
    ))
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(entry_id, corrected, published_at)| Updated {
            entry_id,
            corrected,
            published_at,
        })
        .collect())
}

/// 記事の原文の更新（公開の確認画面で使う。元なし・記事がなければ None）。
pub async fn source_update_of(
    conn: &mut PgConnection,
    entry_id: Uuid,
) -> Result<Option<SourceUpdate>> {
    let row: Option<(Uuid, Option<Uuid>)> = sqlx::query_as(
        "SELECT s.id, e.reflected_source_version_id FROM entries e
         JOIN entries s ON s.id = e.source_entry_id AND s.deleted_at IS NULL
         WHERE e.id = $1 AND e.deleted_at IS NULL",
    )
    .bind(entry_id)
    .fetch_optional(&mut *conn)
    .await?;
    match row {
        Some((source, reflected)) => source_update(conn, source, reflected).await,
        None => Ok(None),
    }
}

/// (翻訳グループ, ロケール, 元の記事, 元の記事のロケール, 反映済みの版)
type InfoRow = (Uuid, String, Option<Uuid>, Option<String>, Option<Uuid>);

/// 翻訳の情報を読む（記事がなければ None）。
pub async fn info(conn: &mut PgConnection, entry_id: Uuid) -> Result<Option<TranslationInfo>> {
    let row: Option<InfoRow> = sqlx::query_as(
        "SELECT e.translation_group_id, e.locale, s.id, s.locale,
                CASE WHEN s.id IS NULL THEN NULL ELSE e.reflected_source_version_id END
         FROM entries e
         LEFT JOIN entries s ON s.id = e.source_entry_id AND s.deleted_at IS NULL
         WHERE e.id = $1 AND e.deleted_at IS NULL",
    )
    .bind(entry_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((group_id, locale, source_entry_id, source_locale, reflected_source_version_id)) = row
    else {
        return Ok(None);
    };
    let members: Vec<(Uuid, String, bool)> = sqlx::query_as(
        "SELECT id, locale, published_version_id IS NOT NULL FROM entries
         WHERE translation_group_id = $1 AND deleted_at IS NULL ORDER BY locale",
    )
    .bind(group_id)
    .fetch_all(&mut *conn)
    .await?;
    let source_update = match source_entry_id {
        Some(s) => source_update(conn, s, reflected_source_version_id).await?,
        None => None,
    };
    Ok(Some(TranslationInfo {
        group_id,
        locale,
        source_entry_id,
        source_locale,
        reflected_source_version_id,
        source_update,
        members: members
            .into_iter()
            .map(|(entry_id, locale, published)| Member {
                entry_id,
                locale,
                published,
            })
            .collect(),
    }))
}

/// 翻訳を作れなかった理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    NotFound,
    /// その翻訳グループに、そのロケールの記事がもうある。
    LocaleExists,
    /// 元の記事の下書きが、いまのスキーマに合わない。
    Invalid(Vec<String>),
}

/// 元の記事を複製して、新しいロケールの記事を作る。
///
/// スラッグは複製するが、新しいロケールで他の記事が使っていれば空にする（下書きなので後から決められる）。
pub async fn create(
    conn: &mut PgConnection,
    manifest: &Manifest,
    source_id: Uuid,
    locale: &str,
    created_by: Uuid,
) -> Result<Result<Draft, Refused>> {
    // 元の記事をロックして、同時に同じロケールの翻訳を作るのを防ぐ
    let source: Option<(Uuid, Option<Uuid>)> = sqlx::query_as(
        "SELECT translation_group_id, published_version_id FROM entries
         WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(source_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((group, source_version)) = source else {
        return Ok(Err(Refused::NotFound));
    };
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM entries WHERE translation_group_id = $1 AND locale = $2 AND deleted_at IS NULL)",
    )
    .bind(group)
    .bind(locale)
    .fetch_one(&mut *conn)
    .await?;
    if exists {
        return Ok(Err(Refused::LocaleExists));
    }
    let Some(draft) = entries::draft(conn, manifest, source_id).await? else {
        return Ok(Err(Refused::NotFound));
    };
    let Some(ct) = manifest.content_type(&draft.content_type) else {
        return Ok(Err(Refused::NotFound));
    };
    let mut columns = match fields::convert_all(manifest, ct, &draft.fields) {
        Ok(c) => c,
        Err(problems) => return Ok(Err(Refused::Invalid(problems))),
    };
    let id = Uuid::new_v4();
    if let Some(slug_field) = ct.fields.iter().find(|f| f.kind == FieldType::Slug)
        && let Some((_, Column::Text(Some(slug)))) =
            columns.iter().find(|(n, _)| *n == slug_field.name)
        && entries::slug_taken(conn, ct, locale, slug, id).await?
    {
        columns.retain(|(n, _)| *n != slug_field.name);
    }
    entries::create(conn, ct, id, locale, group, created_by, &columns).await?;
    sqlx::query(
        "UPDATE entries SET source_entry_id = $2, reflected_source_version_id = $3 WHERE id = $1",
    )
    .bind(id)
    .bind(source_id)
    .bind(source_version)
    .execute(&mut *conn)
    .await?;
    let draft = entries::draft(conn, manifest, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("created translation is missing"))?;
    Ok(Ok(draft))
}

/// 元の記事の、反映済みの版から今の版までの差分。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceDiff {
    /// 元の記事（元なしなら null）。
    pub source_entry_id: Option<Uuid>,
    /// 比べ元（反映済みの版）と比べ先（元の記事のいちばん新しい版）。
    pub from_version_id: Option<Uuid>,
    pub to_version_id: Option<Uuid>,
    /// 差分。比べる版がない、または版の本文が抹消されていれば null（理由は `unavailable`）。
    pub diff: Option<Diff>,
    /// 差分を出せない理由（"no_source"、"not_reflected"、"no_version"、"erased"）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diff {
    /// 本文以外で変わったフィールドの名前（スキーマの順）。
    pub changed_fields: Vec<String>,
    /// 変わったセクション（本文の順。導入が先）。
    pub sections: Vec<SectionDiff>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionDiff {
    /// 見出しのアンカーID（導入は null）。
    pub anchor: Option<String>,
    /// 導入のときの本文のフィールドの名前。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// 見出しの文字列（導入は null）。
    pub title: Option<String>,
    pub kind: ChangeKind,
    /// そのセクションの前後の文字列（ないほうは null）。
    pub before: Option<String>,
    pub after: Option<String>,
}

/// 版のフィールド（抹消されていれば None）。
async fn version_fields(
    conn: &mut PgConnection,
    version_id: Uuid,
) -> Result<Option<Map<String, Value>>> {
    let text: Option<Option<String>> =
        sqlx::query_scalar("SELECT fields::text FROM versions WHERE id = $1")
            .bind(version_id)
            .fetch_optional(&mut *conn)
            .await?;
    Ok(match text.flatten() {
        Some(t) => Some(serde_json::from_str(&t)?),
        None => None,
    })
}

/// 元の記事の、反映済みの版から今の版までの差分を求める（記事がなければ None）。
pub async fn source_diff(
    conn: &mut PgConnection,
    manifest: &Manifest,
    entry_id: Uuid,
) -> Result<Option<SourceDiff>> {
    let Some(t) = info(conn, entry_id).await? else {
        return Ok(None);
    };
    let unavailable = |from, to, reason| SourceDiff {
        source_entry_id: t.source_entry_id,
        from_version_id: from,
        to_version_id: to,
        diff: None,
        unavailable: Some(reason),
    };
    let Some(source) = t.source_entry_id else {
        return Ok(Some(unavailable(None, None, "no_source")));
    };
    let from = t.reflected_source_version_id;
    let Some(to) = latest_version_of(conn, source).await? else {
        return Ok(Some(unavailable(from, None, "no_version")));
    };
    let Some(from_id) = from else {
        return Ok(Some(unavailable(None, Some(to), "not_reflected")));
    };
    let (Some(before), Some(after)) = (
        version_fields(conn, from_id).await?,
        version_fields(conn, to).await?,
    ) else {
        return Ok(Some(unavailable(from, Some(to), "erased")));
    };
    // 本文のフィールドは、元の記事のいまのスキーマで決める
    let Some(ct) = entries::draft(conn, manifest, source)
        .await?
        .and_then(|d| manifest.content_type(&d.content_type))
    else {
        return Ok(None);
    };
    let changed_fields = ct
        .fields
        .iter()
        .filter(|f| f.kind != FieldType::Richtext && before.get(&f.name) != after.get(&f.name))
        .map(|f| f.name.clone())
        .collect();
    let before_fields = rich_text_fields(ct, &before);
    let after_fields = rich_text_fields(ct, &after);
    let before_docs: Vec<&Value> = before_fields.iter().map(|(_, v)| *v).collect();
    let after_docs: Vec<&Value> = after_fields.iter().map(|(_, v)| *v).collect();
    let sections = sections::compare(&before_docs, &after_docs)
        .into_iter()
        .map(|c| SectionDiff {
            field: c.lead_of.and_then(|i| {
                after_fields
                    .get(i)
                    .or(before_fields.get(i))
                    .map(|(n, _)| (*n).to_owned())
            }),
            before: sections::section_text(&before_docs, &c),
            after: sections::section_text(&after_docs, &c),
            anchor: c.anchor,
            title: c.title,
            kind: c.kind,
        })
        .collect();
    Ok(Some(SourceDiff {
        source_entry_id: Some(source),
        from_version_id: from,
        to_version_id: Some(to),
        diff: Some(Diff {
            changed_fields,
            sections,
        }),
        unavailable: None,
    }))
}

/// 反映済みの記録を変えられなかった理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReflectRefused {
    NotFound,
    /// 元なし。
    NoSource,
}

/// 「原文の更新を反映済み」: 反映済みの版を、元の記事のいちばん新しい版に進める（Q162, Q164）。
/// 進めた版を返す（元の記事に版がなければ None のまま）。
pub async fn mark_reflected(
    conn: &mut PgConnection,
    entry_id: Uuid,
) -> Result<Result<Option<Uuid>, ReflectRefused>> {
    let row: Option<(Option<Uuid>,)> = sqlx::query_as(
        "SELECT s.id FROM entries e
         LEFT JOIN entries s ON s.id = e.source_entry_id AND s.deleted_at IS NULL
         WHERE e.id = $1 AND e.deleted_at IS NULL FOR UPDATE OF e",
    )
    .bind(entry_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((source,)) = row else {
        return Ok(Err(ReflectRefused::NotFound));
    };
    let Some(source) = source else {
        return Ok(Err(ReflectRefused::NoSource));
    };
    let latest = latest_version_of(conn, source).await?;
    sqlx::query("UPDATE entries SET reflected_source_version_id = $2 WHERE id = $1")
        .bind(entry_id)
        .bind(latest)
        .execute(conn)
        .await?;
    Ok(Ok(latest))
}

/// 元の記事を設定し直せなかった理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetSourceRefused {
    NotFound,
    /// 元にする記事がない（削除済みを含む）か、別の翻訳グループの記事か、この記事自身。
    InvalidSource,
}

/// 元の記事を、同じ翻訳グループの別の記事に設定し直す（Q156）。反映済みの版は、その記事のいちばん新しい版にする。
pub async fn set_source(
    conn: &mut PgConnection,
    entry_id: Uuid,
    source_id: Uuid,
) -> Result<Result<(), SetSourceRefused>> {
    let group: Option<Uuid> = sqlx::query_scalar(
        "SELECT translation_group_id FROM entries WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(entry_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(group) = group else {
        return Ok(Err(SetSourceRefused::NotFound));
    };
    let ok: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM entries
           WHERE id = $1 AND translation_group_id = $2 AND deleted_at IS NULL AND id <> $3)",
    )
    .bind(source_id)
    .bind(group)
    .bind(entry_id)
    .fetch_one(&mut *conn)
    .await?;
    if !ok {
        return Ok(Err(SetSourceRefused::InvalidSource));
    }
    let latest = latest_version_of(conn, source_id).await?;
    sqlx::query(
        "UPDATE entries SET source_entry_id = $2, reflected_source_version_id = $3 WHERE id = $1",
    )
    .bind(entry_id)
    .bind(source_id)
    .bind(latest)
    .execute(conn)
    .await?;
    Ok(Ok(()))
}
