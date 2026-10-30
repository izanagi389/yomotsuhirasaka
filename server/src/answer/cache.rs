//! 回答キャッシュ（09-qa-chatbot.md「回答キャッシュ」、Q29, Q31, Q45, Q134）。
//!
//! - 完全一致のキャッシュ。キーは、正規化してマスクした質問文・ワークスペース・ロケール・設定（モデル・プロンプトの形式・
//!   検索の対象のフィールドの決め方）を、
//!   サーバーの鍵から導いた鍵で HMAC-SHA256 したもの。質問文の平文は保存しない（INV-QA-08）。DBを読めても、よくある質問を
//!   試して当てられないよう、鍵付きにする。
//! - 保存するのは、引用の照合を通った「回答できた」最終判定だけ（違反を検知したもの・「情報なし」・エラーは保存しない）。
//! - 出典は、記事・版・ロケール・アンカー・パンくずだけを保存する。URLと鮮度は保存せず、返すときに今の状態から引く（Q39, Q45）。
//! - 返す前に、出典ごとに今の状態と照合する（原則3）。1つでも合わなければ、消して作り直す（INV-QA-06, 07）。
//!   - 記事が削除されておらず、公開中の版がキャッシュの版と同じで、本文が抹消されておらず、チャットボットのスイッチが ON
//!   - 既定のロケールへのフォールバックで使った出典なら、質問のロケールの見せてよい翻訳が今もない
//!   - 確認期限が、キャッシュを作った時刻から今までの間に来ていない（期限をまたいだキャッシュは捨てる。INV-QA-09）
//! - 版の本文を抹消したら、その版を引用したキャッシュを消す（[`forget_version`]、Q134）。
//! - 寿命は [`TTL_HOURS`]。ワークスペースごとに [`MAX_ROWS`] 件まで（超えたら古いものから消す）。

use anyhow::Result;
use hmac::{KeyInit as _, Mac as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use super::Source;
use crate::content::freshness;
use crate::schema::manifest::Manifest;
use crate::search::access;
use crate::workspace_db;

/// キャッシュの寿命（時間）。
pub const TTL_HOURS: i32 = 24;
/// ワークスペースごとの件数の上限。
pub const MAX_ROWS: i64 = 10_000;
/// キーを作る鍵の用途の名前（`KeyRing::derive`）。
pub const KEY_PURPOSE: &str = "answer-cache";
/// プロンプトの組み立て方の版。プロンプトや検査を変えて、同じ質問でも回答が変わるときに上げる（設定のハッシュに入る）。
const PROMPT_VERSION: &str = "answer-prompt-1";

/// 質問文の正規化（空白をまとめて前後を切り、全角の英数字・記号・空白を半角にして、小文字にする）。
/// 正規化は狭くとどめる（意味の違う質問が同じキャッシュに当たらないように）。
pub fn normalize(question: &str) -> String {
    let mut out = String::with_capacity(question.len());
    let mut space = false;
    for c in question.chars() {
        let c = match c {
            '\u{3000}' => ' ',
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            _ => c,
        };
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.extend(c.to_lowercase());
    }
    out
}

/// 回答を左右する設定のハッシュ（回答のモデル・プロンプトの組み立て方・回答の形式・検索の対象のフィールドの決め方）。
/// 検索の対象のフィールドを変えたら、前の決め方のチャンクから作った回答を返さない（検索と同じ。INV-SRC-03）。
pub fn settings(chat_model: &str, manifest: &Manifest) -> String {
    let (types, keys) = access::current_keys(manifest);
    let mut hasher = Sha256::new()
        .chain_update(PROMPT_VERSION.as_bytes())
        .chain_update([0])
        .chain_update(chat_model.as_bytes())
        .chain_update([0])
        .chain_update(super::format::FORMAT_JSON.as_bytes());
    for (t, k) in types.iter().zip(&keys) {
        hasher.update([0]);
        hasher.update(t.as_bytes());
        hasher.update([0]);
        hasher.update(k.as_bytes());
    }
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// キャッシュのキー。各項目は長さを前に付けて区切る（項目の境目をずらして、別のキーと同じにさせない）。
pub fn key(
    secret: &[u8; 32],
    workspace: Uuid,
    locale: &str,
    settings: &str,
    normalized: &str,
) -> Vec<u8> {
    let mut mac =
        hmac::Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(workspace.as_bytes());
    for part in [locale, settings, normalized] {
        mac.update(&(part.len() as u64).to_be_bytes());
        mac.update(part.as_bytes());
    }
    mac.finalize().into_bytes().to_vec()
}

/// 保存する出典（URLと鮮度は保存しない）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Cited {
    entry_id: Uuid,
    version_id: Uuid,
    locale: String,
    anchor: Option<String>,
    breadcrumb: Vec<String>,
}

/// キャッシュから返す回答（出典は今の状態で引いたもの）。
pub struct Hit {
    pub text: String,
    pub truncated: bool,
    pub sources: Vec<Source>,
}

/// キャッシュを引き、出典を今の状態と照合する。合わなければ消して None。
pub async fn lookup(
    conn: &mut PgConnection,
    manifest: &Manifest,
    key: &[u8],
    question_locale: &str,
) -> Result<Option<Hit>> {
    let row: Option<(String, bool, String)> = sqlx::query_as(
        "SELECT text, truncated, sources::text FROM answer_cache WHERE key = $1 AND expires_at > now()",
    )
    .bind(key)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((text, truncated, sources_json)) = row else {
        return Ok(None);
    };
    let cited: Vec<Cited> = serde_json::from_str(&sources_json)?;
    let mut sources = Vec::with_capacity(cited.len());
    for c in cited {
        match current(conn, manifest, key, question_locale, c).await? {
            Some(s) => sources.push(s),
            None => {
                sqlx::query("DELETE FROM answer_cache WHERE key = $1")
                    .bind(key)
                    .execute(&mut *conn)
                    .await?;
                return Ok(None);
            }
        }
    }
    if sources.is_empty() {
        return Ok(None);
    }
    Ok(Some(Hit {
        text,
        truncated,
        sources,
    }))
}

/// 出典1つを今の状態と照合し、今の公開URL・鮮度を付ける。見せられなくなった・版が変わった・期限をまたいだなら None。
async fn current(
    conn: &mut PgConnection,
    manifest: &Manifest,
    key: &[u8],
    question_locale: &str,
    c: Cited,
) -> Result<Option<Source>> {
    // 記事の条件は検索と同じ照合（共通の層）。公開中の版が、キャッシュの版と同じであること
    let visible = access::visible_entry(3);
    let untranslated = access::untranslated(4);
    let row: Option<(Option<String>, String, bool)> = sqlx::query_as(crate::db::trusted(format!(
        "SELECT e.url, e.content_type, {untranslated} FROM entries e
         WHERE e.id = $1 AND e.published_version_id = $2 AND {visible}"
    )))
    .bind(c.entry_id)
    .bind(c.version_id)
    .bind(&c.locale)
    // 既定のロケールへのフォールバックで使った出典だけ、翻訳がないことを確かめる
    .bind((c.locale != question_locale).then_some(question_locale))
    .fetch_optional(&mut *conn)
    .await?;
    let Some((url, content_type, untranslated)) = row else {
        return Ok(None);
    };
    if !untranslated {
        return Ok(None);
    }
    let days = manifest
        .content_type(&content_type)
        .and_then(|ct| ct.review.as_ref())
        .map(|r| r.interval_days);
    let fresh = freshness::get(conn, days, c.entry_id).await?;
    let (last, expired, due) = fresh.map_or((None, false, None), |f| {
        (f.last_verified_at, f.review_expired, f.review_due_at)
    });
    // 確認期限が、キャッシュを作った時刻から今までの間に来たなら、捨てる（期限切れの状態は保存しない）
    if let Some(due) = due {
        let crossed: Option<bool> = sqlx::query_scalar(
            "SELECT $2::timestamptz > created_at AND $2::timestamptz <= now() FROM answer_cache WHERE key = $1",
        )
        .bind(key)
        .bind(&due)
        .fetch_optional(&mut *conn)
        .await?;
        if crossed != Some(false) {
            return Ok(None);
        }
    }
    Ok(Some(Source {
        entry_id: c.entry_id,
        locale: c.locale,
        url: url.filter(|u| !u.is_empty()),
        anchor: c.anchor,
        breadcrumb: c.breadcrumb,
        version_id: c.version_id,
        last_reviewed_at: last,
        review_expired: expired,
    }))
}

/// 回答を保存する（同じキーがあれば置き換える）。件数の上限を超えた分は、古いものから消す。
pub async fn store(
    conn: &mut PgConnection,
    key: &[u8],
    text: &str,
    truncated: bool,
    sources: &[Source],
) -> Result<()> {
    let cited: Vec<Cited> = sources
        .iter()
        .map(|s| Cited {
            entry_id: s.entry_id,
            version_id: s.version_id,
            locale: s.locale.clone(),
            anchor: s.anchor.clone(),
            breadcrumb: s.breadcrumb.clone(),
        })
        .collect();
    sqlx::query(
        "INSERT INTO answer_cache (key, text, truncated, sources, expires_at)
         VALUES ($1, $2, $3, $4::jsonb, now() + make_interval(hours => $5))
         ON CONFLICT (key) DO UPDATE SET text = EXCLUDED.text, truncated = EXCLUDED.truncated,
           sources = EXCLUDED.sources, created_at = now(), expires_at = EXCLUDED.expires_at",
    )
    .bind(key)
    .bind(text)
    .bind(truncated)
    .bind(serde_json::to_string(&cited)?)
    .bind(TTL_HOURS)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "DELETE FROM answer_cache WHERE key IN (
           SELECT key FROM answer_cache ORDER BY created_at DESC, key OFFSET $1)",
    )
    .bind(MAX_ROWS)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// 版を引用したキャッシュを消す（版の本文を抹消したとき。Q134）。
pub async fn forget_version(conn: &mut PgConnection, version_id: Uuid) -> Result<()> {
    sqlx::query(
        "DELETE FROM answer_cache WHERE sources @> jsonb_build_array(jsonb_build_object('versionId', $1::text))",
    )
    .bind(version_id.to_string())
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// 寿命の過ぎたキャッシュを、すべてのワークスペースで消す（常駐の処理から1時間ごとに呼ぶ）。
pub async fn sweep(pool: &PgPool) -> Result<u64> {
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM cms.workspaces ORDER BY created_at")
        .fetch_all(pool)
        .await?;
    let mut removed = 0;
    for id in ids {
        let Some(db) = workspace_db::lookup(pool, id).await? else {
            continue;
        };
        let mut tx = workspace_db::begin(pool, &db).await?;
        removed += sqlx::query("DELETE FROM answer_cache WHERE expires_at <= now()")
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_is_narrow() {
        assert_eq!(normalize("  Ｈｅｌｌｏ　 World！ "), "hello world!");
        assert_eq!(normalize("ＡＢＣ\tdef\n\nghi"), "abc def ghi");
        // 全角の英数字・記号・空白のほかは変えない（半角カナ・丸数字・ひらがななど）
        assert_eq!(normalize("ｶﾅ ① かな"), "ｶﾅ ① かな");
    }

    #[test]
    fn keys_depend_on_every_part_and_the_secret() {
        let ws = Uuid::new_v4();
        let k = |secret: &[u8; 32], ws, l: &str, s: &str, q: &str| key(secret, ws, l, s, q);
        let a = [1u8; 32];
        let base = k(&a, ws, "ja", "s", "q");
        assert_eq!(base, k(&a, ws, "ja", "s", "q"));
        assert_ne!(base, k(&[2u8; 32], ws, "ja", "s", "q"));
        assert_ne!(base, k(&a, Uuid::new_v4(), "ja", "s", "q"));
        assert_ne!(base, k(&a, ws, "en", "s", "q"));
        assert_ne!(base, k(&a, ws, "ja", "t", "q"));
        assert_ne!(base, k(&a, ws, "ja", "s", "r"));
        // 項目の境目をずらしても、同じキーにならない
        assert_ne!(k(&a, ws, "ja", "sq", ""), k(&a, ws, "ja", "s", "q"));
        let m = crate::schema::manifest::tests::sample();
        assert_ne!(settings("model-a", &m), settings("model-b", &m));
        // 検索の対象のフィールドを変えたら、設定も変わる
        let mut changed = m.clone();
        for ct in &mut changed.content_types {
            for f in &mut ct.fields {
                f.searchable = !f.searchable;
            }
        }
        assert_ne!(settings("model-a", &m), settings("model-a", &changed));
    }
}
