//! 関連記事（08-search-and-related.md「関連記事」、Q60〜Q62、M3）。
//!
//! - 記事全体のベクトルは、公開中の版のチャンクの埋め込み（ワークスペースが今使っているモデル）の平均を、長さ1にそろえたもの。
//!   追加のLLMの費用はかからない。チャンクの埋め込みがそろっていない記事は、ベクトルを作らない（関連記事は空、候補にもならない）。
//! - 関連記事の候補は、同じコンテンツタイプ・同じロケールの、ほかの翻訳グループの記事から、似ている順に [`STORED`] 件を保存する。
//!   - 候補どうしの類似度が [`DUPLICATE_SIMILARITY`] 以上なら、最終確認日（なければ最初の公開日）が新しいほうだけを残す。
//!   - 確認期限切れの記事は、点数に [`EXPIRED_FACTOR`] をかけて順位を下げる（除外はしない）。
//! - 計算するのは、検索用データのジョブで記事のベクトルを作り直したとき（公開・更新のとき）と、固定・除外を変えたとき。
//!   ほかの記事の一覧に新しい記事が入るのは、毎日の一括計算のとき。
//! - 保存した候補は、返すときに公開状態・ロケール・除外を毎回照合する（`access::visible_entry`。INV-REL-01）。

use std::collections::HashMap;

use anyhow::Result;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use super::access;
use crate::db::Ident;
use crate::schema::manifest::Manifest;
use crate::workspace_db::{self, WorkspaceDb};

/// 記事ごとに保存する候補の数（返す数の上限の3倍。照合で落ちても短くなりにくいように）。
pub const STORED: usize = 30;
/// 返す数の既定と上限。
pub const DEFAULT_FIRST: i64 = 5;
pub const MAX_FIRST: i64 = 10;
/// ほぼ同じ内容とみなす類似度。
const DUPLICATE_SIMILARITY: f64 = 0.97;
/// 確認期限切れの記事の点数にかける値。
const EXPIRED_FACTOR: f64 = 0.9;
/// 補正の前に、似ている順に読む候補の数。
const FETCHED: i64 = 60;
/// 1回に読む記事の数。
const PAGE: i64 = 200;

/// 候補（補正の前）。
#[derive(Debug, Clone)]
pub struct Candidate {
    pub entry_id: Uuid,
    /// 元の記事とのコサイン類似度。
    pub similarity: f64,
    /// 最終確認日（なければ最初の公開日）の UNIX 時刻。新しいほど大きい。
    pub last_verified: f64,
    pub expired: bool,
    pub vector: Vec<f32>,
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    // どちらも長さ1にそろえてあるので、内積がコサイン類似度
    a.iter()
        .zip(b)
        .map(|(x, y)| f64::from(*x) * f64::from(*y))
        .sum()
}

/// 埋め込みの平均を、長さ1にそろえる（空、または長さが0なら None）。
pub fn mean_normalized(vectors: &[Vec<f32>]) -> Option<Vec<f32>> {
    let dims = vectors.first()?.len();
    let mut sum = vec![0f64; dims];
    for v in vectors {
        if v.len() != dims {
            return None;
        }
        for (s, x) in sum.iter_mut().zip(v) {
            *s += f64::from(*x);
        }
    }
    let norm = sum.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm == 0.0 || !norm.is_finite() {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    Some(sum.into_iter().map(|x| (x / norm) as f32).collect())
}

/// 候補を補正して並べる（似ている順の候補から、保存する [`STORED`] 件と点数）。
pub fn rank(candidates: Vec<Candidate>) -> Vec<(Uuid, f64)> {
    let mut kept: Vec<Candidate> = Vec::new();
    for c in candidates {
        // ほぼ同じ内容の候補が既にあれば、最終確認日が新しいほうだけを残す
        match kept
            .iter()
            .position(|k| cosine(&k.vector, &c.vector) >= DUPLICATE_SIMILARITY)
        {
            Some(i) if c.last_verified > kept[i].last_verified => kept[i] = c,
            Some(_) => {}
            None => kept.push(c),
        }
    }
    let mut ranked: Vec<(Uuid, f64)> = kept
        .into_iter()
        .map(|c| {
            let score = if c.expired {
                c.similarity * EXPIRED_FACTOR
            } else {
                c.similarity
            };
            (c.entry_id, score)
        })
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    ranked.truncate(STORED);
    ranked
}

/// 記事のベクトルを今の公開中の版とモデルにそろえ、作り直した記事と、一覧のない記事の関連記事を計算する（検索用データのジョブから）。
/// `model`・`dims` はワークスペースが今使っているモデル。外部とは通信しない。
pub async fn refresh(
    pool: &PgPool,
    db: &WorkspaceDb,
    manifest: &Manifest,
    vs: &Ident,
    model: &str,
    dims: u32,
) -> Result<()> {
    let d = i32::try_from(dims).unwrap_or(i32::MAX);
    let mut tx = workspace_db::begin(pool, db).await?;
    // 今のモデルでない・公開中の版のものでないベクトルと、ベクトルのない記事の一覧を消す
    sqlx::query(
        "DELETE FROM search_entry_vectors v
         WHERE (v.model, v.dimensions) <> ($1, $2)
            OR NOT EXISTS (SELECT 1 FROM entries e
                           WHERE e.id = v.entry_id AND e.published_version_id = v.version_id AND e.deleted_at IS NULL)",
    )
    .bind(model)
    .bind(d)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM related_entries r
         WHERE NOT EXISTS (SELECT 1 FROM search_entry_vectors v WHERE v.entry_id = r.entry_id)",
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    // ベクトルを作る記事（公開中の版のチャンクがあり、どれにも今のモデルの埋め込みがあって、ベクトルがまだないもの）
    let mut changed: Vec<Uuid> = Vec::new();
    loop {
        let mut tx = workspace_db::begin(pool, db).await?;
        let rows: Vec<(Uuid, Uuid)> = sqlx::query_as(
            "SELECT c.entry_id, c.version_id
             FROM search_chunks c JOIN entries e ON e.id = c.entry_id AND e.published_version_id = c.version_id
             WHERE NOT EXISTS (SELECT 1 FROM search_entry_vectors v
                               WHERE v.entry_id = c.entry_id AND v.version_id = c.version_id
                                 AND v.model = $1 AND v.dimensions = $2)
             GROUP BY c.entry_id, c.version_id
             HAVING bool_and(EXISTS (SELECT 1 FROM search_embeddings x
                                     WHERE x.content_hash = c.content_hash AND x.model = $1 AND x.dimensions = $2))
             ORDER BY c.entry_id
             LIMIT $3",
        )
        .bind(model)
        .bind(d)
        .bind(PAGE)
        .fetch_all(&mut *tx)
        .await?;
        if rows.is_empty() {
            tx.commit().await?;
            break;
        }
        for (entry_id, version_id) in rows {
            let vectors: Vec<Vec<f32>> = sqlx::query_scalar(
                "SELECT x.embedding FROM search_chunks c
                 JOIN search_embeddings x ON x.content_hash = c.content_hash AND x.model = $3 AND x.dimensions = $4
                 WHERE c.entry_id = $1 AND c.version_id = $2",
            )
            .bind(entry_id)
            .bind(version_id)
            .bind(model)
            .bind(d)
            .fetch_all(&mut *tx)
            .await?;
            let Some(vector) = mean_normalized(&vectors) else {
                continue;
            };
            sqlx::query(
                "INSERT INTO search_entry_vectors (entry_id, version_id, model, dimensions, vector)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (entry_id, model, dimensions)
                 DO UPDATE SET version_id = $2, vector = $5, updated_at = now()",
            )
            .bind(entry_id)
            .bind(version_id)
            .bind(model)
            .bind(d)
            .bind(&vector)
            .execute(&mut *tx)
            .await?;
            changed.push(entry_id);
        }
        tx.commit().await?;
    }

    // 作り直した記事と、一覧のない記事の関連記事を計算する
    let mut tx = workspace_db::begin(pool, db).await?;
    let missing: Vec<Uuid> = sqlx::query_scalar(
        "SELECT v.entry_id FROM search_entry_vectors v
         WHERE NOT EXISTS (SELECT 1 FROM related_entries r WHERE r.entry_id = v.entry_id)",
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    changed.extend(missing);
    changed.sort();
    changed.dedup();
    for entry_id in changed {
        let mut tx = workspace_db::begin(pool, db).await?;
        compute(&mut tx, manifest, vs, model, dims, entry_id).await?;
        tx.commit().await?;
    }
    Ok(())
}

/// 1つの記事の関連記事を計算して保存する。一覧が前と変わったら true（ベクトルがなければ、一覧を消して false）。
/// `conn` はワークスペースのロールとスキーマに切り替えたトランザクション。
pub async fn compute(
    conn: &mut PgConnection,
    manifest: &Manifest,
    vs: &Ident,
    model: &str,
    dims: u32,
    entry_id: Uuid,
) -> Result<bool> {
    let d = i32::try_from(dims).unwrap_or(i32::MAX);
    let me: Option<(Vec<f32>, String, String, Uuid)> = sqlx::query_as(
        "SELECT v.vector, e.content_type, e.locale, e.translation_group_id
         FROM search_entry_vectors v JOIN entries e ON e.id = v.entry_id
         WHERE v.entry_id = $1 AND v.model = $2 AND v.dimensions = $3",
    )
    .bind(entry_id)
    .bind(model)
    .bind(d)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((vector, content_type, locale, group)) = me else {
        sqlx::query("DELETE FROM related_entries WHERE entry_id = $1")
            .bind(entry_id)
            .execute(&mut *conn)
            .await?;
        return Ok(false);
    };
    let default_days = manifest
        .content_type(&content_type)
        .and_then(|ct| ct.review.as_ref())
        .map(|r| i32::try_from(r.interval_days).unwrap_or(i32::MAX));
    // 同じコンテンツタイプ・同じロケールの、ほかの翻訳グループの記事を、似ている順に（見せてよい記事だけ）
    let visible = access::visible_entry(4);
    let distance = format!(
        "(o.vector::{vs}.vector({dims})) OPERATOR({vs}.<=>) (($1::real[])::{vs}.vector({dims}))"
    );
    let rows: Vec<CandidateRow> = sqlx::query_as(crate::db::trusted(format!(
        "SELECT o.entry_id, (1 - ({distance}))::float8, o.vector,
                extract(epoch FROM COALESCE(e.last_verified_at,
                    (SELECT min(published_at) FROM versions WHERE entry_id = e.id)))::float8,
                COALESCE(
                    (CASE WHEN e.review_disabled THEN NULL ELSE COALESCE(e.review_interval_days, $7) END) IS NOT NULL
                    AND COALESCE(e.last_verified_at, (SELECT min(published_at) FROM versions WHERE entry_id = e.id))
                        + make_interval(days => CASE WHEN e.review_disabled THEN NULL
                                                     ELSE COALESCE(e.review_interval_days, $7) END) < now(),
                    false)
         FROM search_entry_vectors o JOIN entries e ON e.id = o.entry_id
         WHERE o.model = $2 AND o.dimensions = $3 AND o.version_id = e.published_version_id
           AND {visible}
           AND e.content_type = $5 AND e.translation_group_id <> $6
         ORDER BY {distance}
         LIMIT $8"
    )))
    .bind(&vector)
    .bind(model)
    .bind(d)
    .bind(&locale)
    .bind(&content_type)
    .bind(group)
    .bind(default_days)
    .bind(FETCHED)
    .fetch_all(&mut *conn)
    .await?;
    let ranked = rank(
        rows.into_iter()
            .map(|(entry_id, similarity, vector, last, expired)| Candidate {
                entry_id,
                similarity,
                last_verified: last.unwrap_or(0.0),
                expired,
                vector,
            })
            .collect(),
    );
    let ids: Vec<Uuid> = ranked.iter().map(|(id, _)| *id).collect();
    #[allow(clippy::cast_possible_truncation)]
    let scores: Vec<f32> = ranked.iter().map(|(_, s)| *s as f32).collect();
    let before: Option<Vec<Uuid>> =
        sqlx::query_scalar("SELECT candidates FROM related_entries WHERE entry_id = $1")
            .bind(entry_id)
            .fetch_optional(&mut *conn)
            .await?;
    sqlx::query(
        "INSERT INTO related_entries (entry_id, candidates, scores) VALUES ($1, $2, $3)
         ON CONFLICT (entry_id) DO UPDATE SET candidates = $2, scores = $3, computed_at = now()",
    )
    .bind(entry_id)
    .bind(&ids)
    .bind(&scores)
    .execute(&mut *conn)
    .await?;
    Ok(before.as_ref() != Some(&ids))
}

/// 候補の行（記事のID・類似度・ベクトル・最終確認日・確認期限切れか）。
type CandidateRow = (Uuid, f64, Vec<f32>, Option<f64>, bool);

/// 返す関連記事の1件（記事のIDと、固定したか、点数）。
#[derive(Debug, Clone, PartialEq)]
pub struct Picked {
    pub entry_id: Uuid,
    pub pinned: bool,
    pub score: f64,
}

/// 記事の関連記事を、返す時点の状態で選ぶ（固定した記事を先に、固定の順で。次に保存した候補を点数の順で）。
/// 固定・候補のどちらも、今の公開状態・ロケール・除外を照合する（INV-REL-01）。
/// 固定は翻訳グループから、同じコンテンツタイプ・同じロケールの、公開中の記事へ解決する。
pub async fn pick(conn: &mut PgConnection, entry_id: Uuid, first: i64) -> Result<Vec<Picked>> {
    let first = usize::try_from(first.clamp(0, MAX_FIRST)).unwrap_or(0);
    let me: Option<(String, String, Uuid)> = sqlx::query_as(
        "SELECT content_type, locale, translation_group_id FROM entries WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(entry_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((content_type, locale, group)) = me else {
        return Ok(Vec::new());
    };
    let (pinned, excluded): (Vec<Uuid>, Vec<Uuid>) =
        sqlx::query_as("SELECT pinned, excluded FROM related_settings WHERE entry_id = $1")
            .bind(entry_id)
            .fetch_optional(&mut *conn)
            .await?
            .unwrap_or_default();
    let (candidates, scores): (Vec<Uuid>, Vec<f32>) =
        sqlx::query_as("SELECT candidates, scores FROM related_entries WHERE entry_id = $1")
            .bind(entry_id)
            .fetch_optional(&mut *conn)
            .await?
            .unwrap_or_default();
    // 固定: 公開中の（削除されていない）記事へ解決する
    let pinned_rows: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT e.translation_group_id, e.id FROM entries e
         WHERE e.translation_group_id = ANY($1) AND e.content_type = $2 AND e.locale = $3
           AND e.deleted_at IS NULL AND e.published_version_id IS NOT NULL AND e.translation_group_id <> $4
           AND EXISTS (SELECT 1 FROM versions v WHERE v.id = e.published_version_id AND v.erased_at IS NULL)",
    )
    .bind(&pinned)
    .bind(&content_type)
    .bind(&locale)
    .bind(group)
    .fetch_all(&mut *conn)
    .await?;
    let by_group: HashMap<Uuid, Uuid> = pinned_rows.into_iter().collect();
    let score_of: HashMap<Uuid, f64> = candidates
        .iter()
        .copied()
        .zip(scores.iter().map(|s| f64::from(*s)))
        .collect();
    let mut out: Vec<Picked> = pinned
        .iter()
        .filter_map(|g| by_group.get(g))
        .map(|id| Picked {
            entry_id: *id,
            pinned: true,
            score: score_of.get(id).copied().unwrap_or(0.0),
        })
        .collect();
    // 候補: 見せてよい記事で、除外・固定した翻訳グループでないもの
    let visible = access::visible_entry(2);
    let ok: Vec<Uuid> = sqlx::query_scalar(crate::db::trusted(format!(
        "SELECT e.id FROM entries e
         WHERE e.id = ANY($1) AND {visible} AND e.content_type = $3
           AND e.translation_group_id <> $4 AND NOT (e.translation_group_id = ANY($5))"
    )))
    .bind(&candidates)
    .bind(&locale)
    .bind(&content_type)
    .bind(group)
    .bind([pinned.as_slice(), excluded.as_slice()].concat())
    .fetch_all(&mut *conn)
    .await?;
    for id in &candidates {
        if out.len() >= first {
            break;
        }
        if ok.contains(id) && !out.iter().any(|p| p.entry_id == *id) {
            out.push(Picked {
                entry_id: *id,
                pinned: false,
                score: score_of.get(id).copied().unwrap_or(0.0),
            });
        }
    }
    out.truncate(first);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(v: &[f32]) -> Vec<f32> {
        mean_normalized(&[v.to_vec()]).unwrap()
    }

    fn cand(id: u128, similarity: f64, last: f64, expired: bool, v: &[f32]) -> Candidate {
        Candidate {
            entry_id: Uuid::from_u128(id),
            similarity,
            last_verified: last,
            expired,
            vector: unit(v),
        }
    }

    #[test]
    fn mean_is_normalized() {
        let m = mean_normalized(&[vec![1.0, 0.0], vec![0.0, 1.0]]).unwrap();
        assert!((cosine(&m, &m) - 1.0).abs() < 1e-6);
        assert!((m[0] - m[1]).abs() < 1e-6);
        assert!(mean_normalized(&[]).is_none());
        assert!(mean_normalized(&[vec![0.0, 0.0]]).is_none());
        assert!(
            mean_normalized(&[vec![1.0], vec![1.0, 2.0]]).is_none(),
            "次元数がそろわない"
        );
    }

    #[test]
    fn near_duplicates_keep_the_newer_one() {
        // 1 と 2 はほぼ同じ内容（2 のほうが新しい）。3 は別の内容
        let ranked = rank(vec![
            cand(1, 0.9, 100.0, false, &[1.0, 0.0, 0.0]),
            cand(2, 0.89, 200.0, false, &[1.0, 0.01, 0.0]),
            cand(3, 0.5, 50.0, false, &[0.0, 1.0, 0.0]),
        ]);
        let ids: Vec<u128> = ranked.iter().map(|(id, _)| id.as_u128()).collect();
        assert_eq!(ids, vec![2, 3]);
        // 古いほうが後に来ても、新しいほうを残す
        let ranked = rank(vec![
            cand(2, 0.9, 200.0, false, &[1.0, 0.0, 0.0]),
            cand(1, 0.89, 100.0, false, &[1.0, 0.01, 0.0]),
        ]);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].0.as_u128(), 2);
    }

    #[test]
    fn expired_entries_rank_lower_but_stay() {
        let ranked = rank(vec![
            cand(1, 0.80, 0.0, true, &[1.0, 0.0]),
            cand(2, 0.75, 0.0, false, &[0.0, 1.0]),
        ]);
        assert_eq!(ranked[0].0.as_u128(), 2);
        assert_eq!(ranked[1].0.as_u128(), 1);
        assert!((ranked[1].1 - 0.72).abs() < 1e-9);
    }

    #[test]
    fn at_most_stored_candidates_are_kept() {
        let many: Vec<Candidate> = (0..50u32)
            .map(|i| {
                let mut v = vec![0.0f32; 64];
                v[i as usize] = 1.0;
                cand(u128::from(i), 1.0 - f64::from(i) / 100.0, 0.0, false, &v)
            })
            .collect();
        assert_eq!(rank(many).len(), STORED);
    }
}
