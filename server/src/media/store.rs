//! メディアのファイルとDBの記録（13-security.md「メディア」、Q96, Q100）。
//!
//! - ファイルは `<保存先>/<ワークスペースIDの32桁>/<ランダムな32桁>.<拡張子>` に置く。名前はすべてサーバーが決め、
//!   利用者が指定したファイル名やパスは使わない（INV-MED-04）。
//! - 書き込みは、同じディレクトリの一時ファイルに書いてから名前を変える（途中までのファイルを配信しない）。
//! - 配信のパスは、決まった形（ワークスペースのID・32桁の16進数・許可した拡張子）だけを受け付ける。
//! - DBの記録はワークスペースのスキーマの `media`（ワークスペースのロールに切り替えたトランザクションの中で扱う）。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

use super::process::{Format, Processed};
use crate::schema::manifest::{FieldType, Manifest};
use crate::schema::migrate::{column_name, table_name};

const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;

/// 保存先のディレクトリを用意する（なければ作る。他の利用者には読ませない）。サーバーの起動時に呼ぶ。
pub fn prepare_root(root: &Path) -> Result<()> {
    std::fs::create_dir_all(root)
        .with_context(|| format!("cannot create media directory {}", root.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("cannot set permissions of {}", root.display()))?;
    }
    Ok(())
}

fn workspace_dir(root: &Path, workspace_id: Uuid) -> PathBuf {
    root.join(workspace_id.simple().to_string())
}

/// 削除したワークスペースのメディアのファイルをすべて消す。まず配信の形に合わない名前へ変えて（すぐに配信されなくなる）、
/// それから中身を消す。ディレクトリがなければ何もしない。
pub async fn remove_workspace(root: &Path, workspace_id: Uuid) -> Result<()> {
    let dir = workspace_dir(root, workspace_id);
    let trash = root.join(format!(".deleted-{}", Uuid::new_v4().simple()));
    match tokio::fs::rename(&dir, &trash).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("cannot remove media of workspace {workspace_id}"));
        }
    }
    tokio::fs::remove_dir_all(&trash)
        .await
        .with_context(|| format!("cannot remove media of workspace {workspace_id}"))
}

/// 保存したファイルの名前として正しい形か（`<32桁の16進数>.<拡張子>`）。
pub fn valid_file_name(name: &str) -> bool {
    let Some((stem, ext)) = name.split_once('.') else {
        return false;
    };
    stem.len() == 32
        && stem
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && Format::from_extension(ext).is_some()
}

/// 配信のときに読むファイルの場所。形が正しくなければ None（保存先の外を指すことはない）。
pub fn served_path(root: &Path, workspace: &str, file: &str) -> Option<(PathBuf, Format)> {
    let ws = Uuid::try_parse(workspace).ok()?;
    // IDの表記ゆれ（大文字やハイフンの有無）を受け付けず、保存に使う形と一致するものだけ
    if ws.simple().to_string() != workspace && ws.hyphenated().to_string() != workspace {
        return None;
    }
    if !valid_file_name(file) {
        return None;
    }
    let format = Format::from_extension(file.rsplit_once('.')?.1)?;
    Some((workspace_dir(root, ws).join(file), format))
}

/// ファイルを書く。書いたファイルの名前を返す。
pub async fn write_file(root: &Path, workspace_id: Uuid, processed: &Processed) -> Result<String> {
    let dir = workspace_dir(root, workspace_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("cannot create media directory {}", dir.display()))?;
    let name = format!(
        "{}.{}",
        Uuid::new_v4().simple(),
        processed.format.extension()
    );
    let tmp = dir.join(format!(".{}.tmp", Uuid::new_v4().simple()));
    let result = async {
        let mut f = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .await?;
        tokio::io::AsyncWriteExt::write_all(&mut f, &processed.bytes).await?;
        f.sync_all().await?;
        drop(f);
        tokio::fs::rename(&tmp, dir.join(&name)).await
    }
    .await;
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e).context("cannot write media file");
    }
    Ok(name)
}

/// ファイルを消す（もうなければ何もしない）。
pub async fn remove_file(root: &Path, workspace_id: Uuid, name: &str) -> Result<()> {
    if !valid_file_name(name) {
        anyhow::bail!("unexpected media file name");
    }
    match tokio::fs::remove_file(workspace_dir(root, workspace_id).join(name)).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).context("cannot remove media file"),
    }
}

/// メディアの情報。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    pub id: Uuid,
    /// 配信のURL。
    pub url: String,
    pub content_type: String,
    pub bytes: i64,
    pub width: i32,
    pub height: i32,
    pub created_by: Option<Uuid>,
    pub created_at: String,
}

type MediaRow = (Uuid, String, String, i64, i32, i32, Option<Uuid>, String);

/// 配信のURL。
pub fn url_of(base: &str, workspace_id: Uuid, file: &str) -> String {
    format!(
        "{}/{}/{file}",
        base.trim_end_matches('/'),
        workspace_id.simple()
    )
}

fn info(base: &str, workspace_id: Uuid, r: MediaRow) -> MediaInfo {
    let (id, file, content_type, bytes, width, height, created_by, created_at) = r;
    MediaInfo {
        id,
        url: url_of(base, workspace_id, &file),
        content_type,
        bytes,
        width,
        height,
        created_by,
        created_at,
    }
}

fn select() -> String {
    format!(
        "SELECT id, file, content_type, bytes, width, height, created_by,
                to_char(created_at AT TIME ZONE 'UTC', {RFC3339}) FROM media"
    )
}

/// 使っている容量（削除していないものの合計）。
pub async fn used_bytes(conn: &mut PgConnection) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(sum(bytes), 0)::bigint FROM media WHERE deleted_at IS NULL",
    )
    .fetch_one(conn)
    .await?)
}

/// 同じワークスペースのアップロードを1つずつにする（容量の確認と記録の間に、他のアップロードが割り込まないように）。
pub async fn lock_uploads(conn: &mut PgConnection) -> Result<()> {
    sqlx::query("LOCK TABLE media IN SHARE ROW EXCLUSIVE MODE")
        .execute(conn)
        .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn insert(
    conn: &mut PgConnection,
    base: &str,
    workspace_id: Uuid,
    id: Uuid,
    file: &str,
    processed: &Processed,
    created_by: Uuid,
) -> Result<MediaInfo> {
    sqlx::query(
        "INSERT INTO media (id, file, content_type, bytes, width, height, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(file)
    .bind(processed.format.content_type())
    .bind(processed.bytes.len() as i64)
    .bind(processed.width as i32)
    .bind(processed.height as i32)
    .bind(created_by)
    .execute(&mut *conn)
    .await?;
    get(conn, base, workspace_id, id)
        .await?
        .context("inserted media is missing")
}

pub async fn get(
    conn: &mut PgConnection,
    base: &str,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<Option<MediaInfo>> {
    let row: Option<MediaRow> = sqlx::query_as(crate::db::trusted(format!(
        "{} WHERE id = $1 AND deleted_at IS NULL",
        select()
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|r| info(base, workspace_id, r)))
}

/// 一覧（新しい順）。
pub async fn list(
    conn: &mut PgConnection,
    base: &str,
    workspace_id: Uuid,
) -> Result<Vec<MediaInfo>> {
    let rows: Vec<MediaRow> = sqlx::query_as(crate::db::trusted(format!(
        "{} WHERE deleted_at IS NULL ORDER BY created_at DESC, id",
        select()
    )))
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| info(base, workspace_id, r))
        .collect())
}

/// メディアを使っている場所。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub entry_id: Uuid,
    pub locale: String,
    /// 下書きで使っているか。
    pub in_draft: bool,
    /// 使っている版の番号（本文を抹消した版は含めない）。
    pub versions: Vec<i32>,
}

/// メディアを使っている記事（削除した記事は含めない）。本文のJSONの中のメディアのIDで探す
/// （本文の検証で、メディアのIDは小文字・ハイフン区切りにそろえてある）。
pub async fn usage(
    conn: &mut PgConnection,
    manifest: &Manifest,
    media_id: Uuid,
) -> Result<Vec<Usage>> {
    let needle = format!("%{}%", media_id.hyphenated());
    let mut found: std::collections::BTreeMap<Uuid, Usage> = Default::default();
    for ct in &manifest.content_types {
        let table = table_name(&ct.name)?;
        // 本文のフィールドの画像と、画像のフィールド（どちらもメディアのIDは小文字・ハイフン区切り）
        for f in ct
            .fields
            .iter()
            .filter(|f| matches!(f.kind, FieldType::Richtext | FieldType::Image))
        {
            let column = column_name(&f.name)?;
            let rows: Vec<(Uuid, String)> = sqlx::query_as(crate::sql!(
                "SELECT e.id, e.locale FROM {table} t JOIN entries e ON e.id = t.entry_id
                 WHERE e.deleted_at IS NULL AND t.{column}::text LIKE $1"
            ))
            .bind(&needle)
            .fetch_all(&mut *conn)
            .await?;
            for (entry_id, locale) in rows {
                found
                    .entry(entry_id)
                    .or_insert_with(|| Usage {
                        entry_id,
                        locale,
                        in_draft: false,
                        versions: Vec::new(),
                    })
                    .in_draft = true;
            }
        }
    }
    let rows: Vec<(Uuid, String, i32)> = sqlx::query_as(
        "SELECT e.id, e.locale, v.number FROM versions v JOIN entries e ON e.id = v.entry_id
         WHERE e.deleted_at IS NULL AND v.erased_at IS NULL AND v.fields::text LIKE $1
         ORDER BY v.number",
    )
    .bind(&needle)
    .fetch_all(&mut *conn)
    .await?;
    for (entry_id, locale, number) in rows {
        found
            .entry(entry_id)
            .or_insert_with(|| Usage {
                entry_id,
                locale,
                in_draft: false,
                versions: Vec::new(),
            })
            .versions
            .push(number);
    }
    Ok(found.into_values().collect())
}

/// ワークスペースにある、削除されていないメディアか（画像のフィールドの保存の検査。INV-IMG-03）。
/// メディアの表はワークスペースのスキーマにあるので、ほかのワークスペースのメディアは見つからない。
pub async fn exists(conn: &mut PgConnection, id: Uuid) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM media WHERE id = $1 AND deleted_at IS NULL)",
    )
    .bind(id)
    .fetch_one(conn)
    .await?)
}

/// 削除の印を付け、ファイルの名前を返す（なければ None）。ファイルはトランザクションの確定の後に消す。
pub async fn mark_deleted(conn: &mut PgConnection, id: Uuid) -> Result<Option<String>> {
    Ok(sqlx::query_scalar(
        "UPDATE media SET deleted_at = now() WHERE id = $1 AND deleted_at IS NULL RETURNING file",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn served_paths_stay_inside_the_root() {
        let root = Path::new("/srv/media");
        let ws = Uuid::new_v4();
        let file = format!("{}.png", Uuid::new_v4().simple());
        let (p, f) = served_path(root, &ws.simple().to_string(), &file).unwrap();
        assert_eq!(p, root.join(ws.simple().to_string()).join(&file));
        assert_eq!(f, Format::Png);
        for (w, f) in [
            (ws.simple().to_string(), "../x.png".to_owned()),
            (
                ws.simple().to_string(),
                format!("{}.svg", Uuid::new_v4().simple()),
            ),
            (
                ws.simple().to_string(),
                format!("{}.PNG", Uuid::new_v4().simple()),
            ),
            (
                ws.simple().to_string(),
                format!("{}.png.png", &Uuid::new_v4().simple().to_string()[..28]),
            ),
            (
                ws.simple().to_string(),
                format!("{}/x.png", Uuid::new_v4().simple()),
            ),
            (
                ws.simple().to_string(),
                format!("{}.png", Uuid::new_v4().simple().to_string().to_uppercase()),
            ),
            ("..".to_owned(), file.clone()),
            (ws.simple().to_string().to_uppercase(), file.clone()),
        ] {
            assert!(served_path(root, &w, &f).is_none(), "{w}/{f}");
        }
    }
}
