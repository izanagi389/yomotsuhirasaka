//! 訂正の説明文・訂正履歴・削除済みアンカー・訂正フィード（06-corrections-and-freshness.md、Q42〜Q48, Q63）。
//! ワークスペースのロールに切り替えたトランザクションの中で呼ぶ。
//!
//! - 説明文は「訂正として公開」した版ごとに持つ。定型文を選んだときは、その時点の文を写して保存する（定型文を直しても変わらない）。
//!   公開した後でも書ける。編集の履歴は管理画面の中だけに残し、配信には出さない（INV-COR-04）。
//! - 説明を書いていない訂正には、ロケールの「説明なしのときの文」（定型文の fallback。初期値は「訂正されました。」）を出す。
//! - 削除済みアンカーは、公開した版から消えたアンカーすべて（訂正でない更新で消えたものも含む）。後で戻ったものは外す。
//!   消えた版が訂正なら、その訂正の説明を添える。本文は出さない（INV-COR-02）。
//! - 訂正フィードには「訂正として公開」した版だけを載せる（INV-COR-03）。公開中の記事だけ（原則3）。

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::schema::manifest::Manifest;

const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;
/// 説明文の長さの上限（文字）。
pub const MAX_NOTE_CHARS: usize = 2000;

/// 公開のときに送る説明（定型文の選択と自由記述。どちらも任意）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NoteInput {
    #[serde(default)]
    pub template_id: Option<Uuid>,
    #[serde(default)]
    pub text: Option<String>,
}

/// 定型文と自由記述から、保存する説明文を作る。定型文が見つからなければ None（誤った指定）。
pub async fn resolve_note(conn: &mut PgConnection, input: &NoteInput) -> Result<Option<String>> {
    let mut parts = Vec::new();
    if let Some(id) = input.template_id {
        let text: Option<String> = sqlx::query_scalar(
            "SELECT text FROM correction_templates WHERE id = $1 AND kind = 'choice'",
        )
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
        match text {
            Some(t) => parts.push(t),
            None => return Ok(None),
        }
    }
    if let Some(t) = input
        .text
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        parts.push(t.to_owned());
    }
    Ok(Some(parts.join("\n")))
}

/// 説明文として受け付けるか。
pub fn valid_note(text: &str) -> bool {
    text.chars().count() <= MAX_NOTE_CHARS && !text.chars().any(|c| c.is_control() && c != '\n')
}

/// 訂正の版の説明を書く（空なら「書いていない」に戻す）。履歴に残す。版が訂正でなければ false。
pub async fn save_note(
    conn: &mut PgConnection,
    entry_id: Uuid,
    version_id: Uuid,
    text: &str,
    by: Option<Uuid>,
) -> Result<bool> {
    let is_correction: Option<bool> = sqlx::query_scalar(
        "SELECT v.correction FROM versions v JOIN entries e ON e.id = v.entry_id
         WHERE v.id = $1 AND v.entry_id = $2 AND e.deleted_at IS NULL",
    )
    .bind(version_id)
    .bind(entry_id)
    .fetch_optional(&mut *conn)
    .await?;
    if is_correction != Some(true) {
        return Ok(false);
    }
    if text.is_empty() {
        sqlx::query("DELETE FROM correction_notes WHERE version_id = $1")
            .bind(version_id)
            .execute(&mut *conn)
            .await?;
    } else {
        sqlx::query(
            "INSERT INTO correction_notes (version_id, text) VALUES ($1, $2)
             ON CONFLICT (version_id) DO UPDATE SET text = EXCLUDED.text",
        )
        .bind(version_id)
        .bind(text)
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query(
        "INSERT INTO correction_note_history (id, version_id, text, edited_by) VALUES ($1, $2, $3, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(version_id)
    .bind(text)
    .bind(by)
    .execute(conn)
    .await?;
    Ok(true)
}

/// 説明の編集の履歴（管理画面だけ）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteEdit {
    pub text: String,
    pub edited_by: Option<Uuid>,
    pub edited_at: String,
}

pub async fn note_history(
    conn: &mut PgConnection,
    entry_id: Uuid,
    version_id: Uuid,
) -> Result<Vec<NoteEdit>> {
    let rows: Vec<(String, Option<Uuid>, String)> = sqlx::query_as(crate::sql!(
        "SELECT h.text, h.edited_by, to_char(h.edited_at AT TIME ZONE 'UTC', {RFC3339})
         FROM correction_note_history h JOIN versions v ON v.id = h.version_id
         WHERE h.version_id = $1 AND v.entry_id = $2 ORDER BY h.edited_at, h.id"
    ))
    .bind(version_id)
    .bind(entry_id)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(text, edited_by, edited_at)| NoteEdit {
            text,
            edited_by,
            edited_at,
        })
        .collect())
}

// ---------------------------------------------------------------------------
// 定型文
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Template {
    #[serde(default)]
    pub id: Option<Uuid>,
    pub locale: String,
    /// `choice`（選べる文）か `fallback`（説明なしのときの文。ロケールに1つ）。
    pub kind: String,
    pub text: String,
}

pub async fn templates(conn: &mut PgConnection) -> Result<Vec<Template>> {
    let rows: Vec<(Uuid, String, String, String)> = sqlx::query_as(
        "SELECT id, locale, kind, text FROM correction_templates ORDER BY locale, kind DESC, position, id",
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, locale, kind, text)| Template {
            id: Some(id),
            locale,
            kind,
            text,
        })
        .collect())
}

/// 定型文の検査（問題の一覧）。
pub fn template_problems(manifest: &Manifest, list: &[Template]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut fallbacks = BTreeMap::new();
    let mut ids = BTreeSet::new();
    if list.len() > 200 {
        problems.push("templates: at most 200".to_owned());
    }
    for (i, t) in list.iter().enumerate() {
        if !manifest.locales.contains(&t.locale) {
            problems.push(format!(
                "templates[{i}].locale: not a locale of this workspace"
            ));
        }
        if t.id.is_some_and(|id| !ids.insert(id)) {
            problems.push(format!("templates[{i}].id: duplicated"));
        }
        match t.kind.as_str() {
            "choice" => {}
            "fallback" => {
                if fallbacks.insert(t.locale.clone(), i).is_some() {
                    problems.push(format!("templates[{i}]: only one fallback per locale"));
                }
            }
            _ => problems.push(format!("templates[{i}].kind: must be choice or fallback")),
        }
        if t.text.trim().is_empty() || !valid_note(&t.text) {
            problems.push(format!(
                "templates[{i}].text: 1 to {MAX_NOTE_CHARS} characters without control characters"
            ));
        }
    }
    problems
}

/// 定型文を置き換える（並びは送られた順）。
pub async fn replace_templates(conn: &mut PgConnection, list: &[Template]) -> Result<()> {
    sqlx::query("DELETE FROM correction_templates")
        .execute(&mut *conn)
        .await?;
    for (i, t) in list.iter().enumerate() {
        sqlx::query(
            "INSERT INTO correction_templates (id, locale, kind, text, position) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(t.id.unwrap_or_else(Uuid::new_v4))
        .bind(&t.locale)
        .bind(&t.kind)
        .bind(t.text.trim())
        .bind(i as i32)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// ロケールの「説明なしのときの文」。
async fn fallback(conn: &mut PgConnection, locale: &str) -> Result<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT text FROM correction_templates WHERE kind = 'fallback' AND locale = $1",
    )
    .bind(locale)
    .fetch_optional(conn)
    .await?)
}

// ---------------------------------------------------------------------------
// 訂正履歴と削除済みアンカー（配信）
// ---------------------------------------------------------------------------

/// 訂正履歴の1件。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Correction {
    pub version_id: Uuid,
    pub corrected_at: String,
    /// 説明（書いていなければ、ロケールの説明なしのときの文）。
    pub description: Option<String>,
    /// 影響したセクションのアンカーID。
    pub sections: Vec<String>,
    /// 説明を書いたか（管理画面の未記入の一覧に使う。配信には出さない）。
    #[serde(skip)]
    pub written: bool,
}

/// 版のメタデータ（古い順）: (版のID, 公開した日時, 訂正か, アンカー, 影響したアンカー, 説明)
type VersionMeta = (
    Uuid,
    String,
    bool,
    Vec<String>,
    Option<Vec<String>>,
    Option<String>,
);

async fn versions_meta(conn: &mut PgConnection, entry_id: Uuid) -> Result<Vec<VersionMeta>> {
    Ok(sqlx::query_as(crate::sql!(
        "SELECT v.id, to_char(v.published_at AT TIME ZONE 'UTC', {RFC3339}), v.correction, v.anchors,
                v.affected_anchors, n.text
         FROM versions v LEFT JOIN correction_notes n ON n.version_id = v.id
         WHERE v.entry_id = $1 ORDER BY v.number"
    ))
    .bind(entry_id)
    .fetch_all(conn)
    .await?)
}

/// 記事の訂正履歴（古い順）。
pub async fn entry_corrections(
    conn: &mut PgConnection,
    entry_id: Uuid,
    locale: &str,
) -> Result<Vec<Correction>> {
    let fallback = fallback(&mut *conn, locale).await?;
    Ok(versions_meta(conn, entry_id)
        .await?
        .into_iter()
        .filter(|(_, _, correction, ..)| *correction)
        .map(
            |(version_id, corrected_at, _, _, affected, note)| Correction {
                version_id,
                corrected_at,
                written: note.is_some(),
                description: note.or_else(|| fallback.clone()),
                sections: affected.unwrap_or_default(),
            },
        )
        .collect())
}

/// 削除済みアンカーの1件。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeletedAnchor {
    pub anchor: String,
    pub deleted_at: String,
    pub description: Option<String>,
}

/// 版の並び（古い順）から、削除済みアンカーを求める。
/// 版: (公開した日時, 訂正か, アンカー, 訂正の説明（説明なしのときの文に置き換え済み）)
fn compute_deleted(versions: &[(String, bool, Vec<String>, Option<String>)]) -> Vec<DeletedAnchor> {
    // アンカー → 最後に消えたときの記録。戻ってきたら消す
    let mut deleted: BTreeMap<String, DeletedAnchor> = BTreeMap::new();
    let mut previous: Option<&Vec<String>> = None;
    for (published_at, correction, anchors, description) in versions {
        for a in anchors {
            deleted.remove(a);
        }
        if let Some(prev) = previous {
            for a in prev.iter().filter(|a| !anchors.contains(a)) {
                deleted.insert(
                    a.clone(),
                    DeletedAnchor {
                        anchor: a.clone(),
                        deleted_at: published_at.clone(),
                        description: if *correction {
                            description.clone()
                        } else {
                            None
                        },
                    },
                );
            }
        }
        previous = Some(anchors);
    }
    let mut out: Vec<DeletedAnchor> = deleted.into_values().collect();
    out.sort_by(|a, b| {
        a.deleted_at
            .cmp(&b.deleted_at)
            .then(a.anchor.cmp(&b.anchor))
    });
    out
}

/// 記事の削除済みアンカー（消えた日時の古い順）。
pub async fn deleted_anchors(
    conn: &mut PgConnection,
    entry_id: Uuid,
    locale: &str,
) -> Result<Vec<DeletedAnchor>> {
    let fallback = fallback(&mut *conn, locale).await?;
    let versions: Vec<_> = versions_meta(conn, entry_id)
        .await?
        .into_iter()
        .map(|(_, at, correction, anchors, _, note)| {
            let description = if correction {
                note.or_else(|| fallback.clone())
            } else {
                None
            };
            (at, correction, anchors, description)
        })
        .collect();
    Ok(compute_deleted(&versions))
}

// ---------------------------------------------------------------------------
// 訂正フィード（配信）と、未記入の訂正（管理画面）
// ---------------------------------------------------------------------------

/// 訂正フィードの1件。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedItem {
    pub entry_id: Uuid,
    pub content_type: String,
    pub url: String,
    pub title: Option<String>,
    pub corrected_at: String,
    pub description: Option<String>,
    pub sections: Vec<String>,
}

type FeedRow = (
    Uuid,
    String,
    Option<String>,
    String,
    Option<Vec<String>>,
    Option<String>,
    Option<String>,
);

/// 記事のタイトル（コンテンツタイプの titleField の値）。
fn title_of(manifest: &Manifest, content_type: &str, fields: Option<&str>) -> Option<String> {
    let field = manifest
        .content_type(content_type)?
        .title_field
        .as_deref()?;
    let fields: Value = serde_json::from_str(fields?).ok()?;
    fields.get(field)?.as_str().map(str::to_owned)
}

/// 訂正フィード（訂正した日時の新しい順）。公開中の記事の、訂正として公開した版だけ。
pub async fn feed(
    conn: &mut PgConnection,
    manifest: &Manifest,
    locale: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<FeedItem>> {
    let fallback = fallback(&mut *conn, locale).await?;
    let rows: Vec<FeedRow> = sqlx::query_as(crate::sql!(
        "SELECT e.id, e.content_type, e.url, to_char(v.published_at AT TIME ZONE 'UTC', {RFC3339}),
                v.affected_anchors, n.text, cur.fields::text
         FROM versions v
         JOIN entries e ON e.id = v.entry_id
         JOIN versions cur ON cur.id = e.published_version_id
         LEFT JOIN correction_notes n ON n.version_id = v.id
         WHERE v.correction AND e.deleted_at IS NULL AND e.locale = $1
         ORDER BY v.published_at DESC, v.id
         LIMIT $2 OFFSET $3"
    ))
    .bind(locale)
    .bind(limit)
    .bind(offset)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(entry_id, content_type, url, corrected_at, affected, note, fields)| FeedItem {
                title: title_of(manifest, &content_type, fields.as_deref()),
                entry_id,
                content_type,
                url: url.unwrap_or_default(),
                corrected_at,
                description: note.or_else(|| fallback.clone()),
                sections: affected.unwrap_or_default(),
            },
        )
        .collect())
}

/// 説明を書いていない訂正（管理画面の一覧。削除していない記事だけ。新しい順）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Unwritten {
    pub entry_id: Uuid,
    pub version_id: Uuid,
    pub locale: String,
    pub title: Option<String>,
    pub corrected_at: String,
}

pub async fn unwritten(conn: &mut PgConnection, manifest: &Manifest) -> Result<Vec<Unwritten>> {
    let rows: Vec<(Uuid, Uuid, String, String, String, Option<String>)> = sqlx::query_as(crate::sql!(
        "SELECT e.id, v.id, e.locale, e.content_type, to_char(v.published_at AT TIME ZONE 'UTC', {RFC3339}),
                v.fields::text
         FROM versions v
         JOIN entries e ON e.id = v.entry_id
         LEFT JOIN correction_notes n ON n.version_id = v.id
         WHERE v.correction AND n.version_id IS NULL AND e.deleted_at IS NULL
         ORDER BY v.published_at DESC, v.id
         LIMIT 500"
    ))
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(entry_id, version_id, locale, content_type, corrected_at, fields)| Unwritten {
                title: title_of(manifest, &content_type, fields.as_deref()),
                entry_id,
                version_id,
                locale,
                corrected_at,
            },
        )
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(
        at: &str,
        correction: bool,
        anchors: &[&str],
        note: Option<&str>,
    ) -> (String, bool, Vec<String>, Option<String>) {
        (
            at.to_owned(),
            correction,
            anchors.iter().map(|a| (*a).to_owned()).collect(),
            note.map(str::to_owned),
        )
    }

    #[test]
    fn deleted_anchors_follow_the_versions() {
        let versions = [
            v("t1", false, &["a", "b", "c"], None),
            // b が通常の更新で消える
            v("t2", false, &["a", "c"], None),
            // c が訂正で消える
            v("t3", true, &["a"], Some("訂正しました")),
            // b が戻る
            v("t4", false, &["a", "b"], None),
        ];
        assert_eq!(
            compute_deleted(&versions),
            vec![DeletedAnchor {
                anchor: "c".into(),
                deleted_at: "t3".into(),
                description: Some("訂正しました".into()),
            }]
        );
    }

    #[test]
    fn notes_are_checked() {
        assert!(valid_note("一行目\n二行目"));
        assert!(!valid_note("tab\there"));
        assert!(!valid_note(&"あ".repeat(MAX_NOTE_CHARS + 1)));
    }
}
