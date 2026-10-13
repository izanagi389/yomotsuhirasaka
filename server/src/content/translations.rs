//! 翻訳の作成と、元の記事・版の記録（03-content-and-editing.md、Q154, Q156, Q159）。
//! ワークスペースのロールに切り替えたトランザクションの中で呼ぶ。
//!
//! - 新しいロケールの記事は、元の記事の下書きを複製して作り、同じ翻訳グループに入れる。
//! - どの記事を元にしたかと、その時点の元の記事の公開中の版を「反映済み」として記録する（後から取り返せない記録、Q159）。
//! - 元の記事が削除されていれば、読むときに「元なし」とする（Q156）。記録そのものは消さない。
//! - 1つの翻訳グループの中で、1つのロケールの記事は1つだけ。

use anyhow::Result;
use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

use super::entries::{self, Draft};
use super::fields::{self, Column};
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
    .fetch_all(conn)
    .await?;
    Ok(Some(TranslationInfo {
        group_id,
        locale,
        source_entry_id,
        source_locale,
        reflected_source_version_id,
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
