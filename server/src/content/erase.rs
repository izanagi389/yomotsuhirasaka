//! 版の本文の抹消（03-content-and-editing.md「版の本文の抹消」、Q134, Q144, Q158）。
//! ワークスペースのロールに切り替えたトランザクションの中で呼ぶ。
//!
//! - 版のIDとメタデータ（番号、日時、URL、アンカーID、訂正の記録）は残し、本文（フィールドのスナップショット）だけを消す。
//!   抹消した版の本文は、どのAPIからも返さない（INV-PUB-04）。
//! - 現在公開中の版は抹消できない（INV-PUB-05）。先に直した版を公開するか、非公開にしてから抹消する。
//! - 抹消と一緒に、指定されたメディアを削除できる。ただし、ワークスペース内のどこか（他の記事・翻訳を含む）の版や下書きで
//!   まだ使われているメディアは、指定されても削除しない（INV-PUB-06）。使われているかは抹消の後の状態で確かめる。

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use super::body;
use crate::media::store;
use crate::schema::manifest::{FieldType, Manifest};

/// 抹消の画面に出すもの。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidates {
    /// この版だけで使われていて、抹消と一緒に削除できるメディア（既定で選択済みにする）。
    pub media: Vec<Uuid>,
    /// 同じ翻訳グループの他のロケールの記事（同じ内容が翻訳されて残っている可能性がある。Q154）。
    pub other_locales: Vec<OtherLocale>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OtherLocale {
    pub entry_id: Uuid,
    pub locale: String,
}

/// 版で使っているメディア（本文のフィールドの画像）。
fn media_in(manifest: &Manifest, content_type: &str, fields: &Value) -> Vec<Uuid> {
    let Some(ct) = manifest.content_type(content_type) else {
        return Vec::new();
    };
    let mut out: Vec<Uuid> = ct
        .fields
        .iter()
        .filter(|f| f.kind == FieldType::Richtext)
        .filter_map(|f| fields.get(&f.name))
        .flat_map(body::media_ids)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// 版の本文と記事の情報（記事が削除されていれば None）。(コンテンツタイプ, 翻訳グループ, 版の番号, 本文)
async fn version_row(
    conn: &mut PgConnection,
    entry_id: Uuid,
    version_id: Uuid,
) -> Result<Option<(String, Uuid, i32, Option<Value>)>> {
    let row: Option<(String, Uuid, i32, Option<String>)> = sqlx::query_as(
        "SELECT e.content_type, e.translation_group_id, v.number, v.fields::text
         FROM versions v JOIN entries e ON e.id = v.entry_id
         WHERE v.entry_id = $1 AND v.id = $2 AND e.deleted_at IS NULL",
    )
    .bind(entry_id)
    .bind(version_id)
    .fetch_optional(conn)
    .await?;
    Ok(match row {
        Some((ct, group, number, fields)) => Some((
            ct,
            group,
            number,
            fields.map(|f| serde_json::from_str(&f)).transpose()?,
        )),
        None => None,
    })
}

/// この版の外で使われていないか（この記事のこの版だけ）。
fn only_here(usage: &[store::Usage], entry_id: Uuid, number: i32) -> bool {
    usage
        .iter()
        .all(|u| u.entry_id == entry_id && !u.in_draft && u.versions.iter().all(|n| *n == number))
}

/// 抹消の候補。版がなければ None。
pub async fn candidates(
    conn: &mut PgConnection,
    manifest: &Manifest,
    entry_id: Uuid,
    version_id: Uuid,
) -> Result<Option<Candidates>> {
    let Some((ct, group, number, fields)) = version_row(conn, entry_id, version_id).await? else {
        return Ok(None);
    };
    let mut media = Vec::new();
    if let Some(fields) = &fields {
        for id in media_in(manifest, &ct, fields) {
            // 削除済みのメディアは候補にしない
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM media WHERE id = $1 AND deleted_at IS NULL)",
            )
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
            if exists && only_here(&store::usage(conn, manifest, id).await?, entry_id, number) {
                media.push(id);
            }
        }
    }
    let other_locales: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, locale FROM entries WHERE translation_group_id = $1 AND id <> $2 AND deleted_at IS NULL ORDER BY locale",
    )
    .bind(group)
    .bind(entry_id)
    .fetch_all(conn)
    .await?;
    Ok(Some(Candidates {
        media,
        other_locales: other_locales
            .into_iter()
            .map(|(entry_id, locale)| OtherLocale { entry_id, locale })
            .collect(),
    }))
}

/// 抹消できなかった理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    NotFound,
    /// 現在公開中の版。
    Published,
}

/// 抹消の結果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Erased {
    pub version_id: Uuid,
    /// 削除したメディア（指定されたもののうち、どこでも使われていなかったもの）。
    pub deleted_media_ids: Vec<Uuid>,
    /// 削除したメディアのファイル名（トランザクションの確定の後に消す。応答には含めない）。
    #[serde(skip)]
    pub deleted_files: Vec<String>,
}

/// 版の本文を抹消し、指定されたメディアのうち、もうどこでも使われていないものに削除の印を付ける。
pub async fn erase(
    conn: &mut PgConnection,
    manifest: &Manifest,
    entry_id: Uuid,
    version_id: Uuid,
    delete_media: &[Uuid],
) -> Result<Result<Erased, Refused>> {
    // 記事をロックし、公開と抹消が入れ違わないようにする
    let current: Option<Option<Uuid>> = sqlx::query_scalar(
        "SELECT published_version_id FROM entries WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(entry_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(current) = current else {
        return Ok(Err(Refused::NotFound));
    };
    if current == Some(version_id) {
        return Ok(Err(Refused::Published));
    }
    let updated = sqlx::query(
        "UPDATE versions SET fields = NULL, rendered = NULL, erased_at = COALESCE(erased_at, now())
         WHERE id = $1 AND entry_id = $2",
    )
    .bind(version_id)
    .bind(entry_id)
    .execute(&mut *conn)
    .await?;
    if updated.rows_affected() == 0 {
        return Ok(Err(Refused::NotFound));
    }
    // その版を引用した回答キャッシュも消す（Q134）
    crate::answer::cache::forget_version(conn, version_id).await?;
    // その版と比べた矛盾の検知の結果も消す（説明に本文の内容が含まれうるため）
    crate::assist::conflicts::forget_version(conn, version_id).await?;
    let mut ids = delete_media.to_vec();
    ids.sort();
    ids.dedup();
    let mut deleted_media_ids = Vec::new();
    let mut deleted_files = Vec::new();
    for id in ids {
        // 抹消の後の状態で、どこでも使われていないものだけを消す
        if !store::usage(conn, manifest, id).await?.is_empty() {
            continue;
        }
        if let Some(file) = store::mark_deleted(conn, id).await? {
            deleted_media_ids.push(id);
            deleted_files.push(file);
        }
    }
    Ok(Ok(Erased {
        version_id,
        deleted_media_ids,
        deleted_files,
    }))
}
