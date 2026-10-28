//! 検索・質問応答・関連記事に共通のアクセス制御の層（08-search-and-related.md「共通の層（Q22）」）。
//!
//! 「見せてよいもの」の判定を2か所に持たないため、検索用データ（チャンク）を使う処理は、すべてここの条件を通す。
//! 判定は、保存済みのデータを信用せず、使うたびに今の状態と照合する（原則3）。
//! - 記事が削除されておらず、そのチャンクの版が今も公開中で、記事ごとのスイッチ（チャットボットに使う）が ON（INV-SRC-01）。
//!   予約公開の版は、公開時刻に公開中の版になるまで、ここを通らない（INV-SRC-02）。
//! - チャンクを作ったときの「検索の対象のフィールド」が、今のスキーマのものと同じ（テンプレートで対象を外した直後も、
//!   古いチャンクを使わない。INV-SRC-03）。
//! - ロケールが一致する。
//! - ワークスペースのスキーマの中だけを読む（ワークスペースのロールとスキーマに切り替えたトランザクションで実行する。INV-SRC-04）。

use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::schema::manifest::{FieldType, Manifest};

/// コンテンツタイプごとの、検索の対象のフィールド（テンプレートの順。richtext か）と、パンくずに使うタイトルのフィールド。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Plan {
    pub fields: Vec<(String, bool)>,
    /// タイトルのフィールド（検索の対象のときだけ。INV-SRC-03）。
    pub title: Option<String>,
}

/// 検索の対象のフィールドのあるコンテンツタイプと、その決め方。
pub fn plans(manifest: &Manifest) -> Vec<(String, Plan)> {
    manifest
        .content_types
        .iter()
        .filter_map(|ct| {
            let fields: Vec<(String, bool)> = ct
                .fields
                .iter()
                .filter(|f| f.searchable)
                .map(|f| (f.name.clone(), f.kind == FieldType::Richtext))
                .collect();
            if fields.is_empty() {
                return None;
            }
            let title = ct
                .title_field
                .clone()
                .filter(|t| ct.fields.iter().any(|f| &f.name == t && f.searchable));
            Some((ct.name.clone(), Plan { fields, title }))
        })
        .collect()
}

/// 決め方の要約（チャンクに残し、使うときに今の決め方と照合する）。
pub fn fields_key(plan: &Plan) -> String {
    let json = serde_json::to_string(plan).expect("plan serializes");
    Sha256::digest(json.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 今のスキーマの（コンテンツタイプ, 決め方の要約）の並び。SQL の `$types`・`$keys` に渡す。
pub fn current_keys(manifest: &Manifest) -> (Vec<String>, Vec<String>) {
    plans(manifest)
        .into_iter()
        .map(|(ct, plan)| {
            let key = fields_key(&plan);
            (ct, key)
        })
        .unzip()
}

/// 見せてよいチャンクの条件。`c` は search_chunks、`e` は entries。
/// 引数: `{types}`・`{keys}` に今のスキーマの並び（[`current_keys`]）、`{locale}` にロケールのパラメーターの番号を入れて使う。
/// キーワード検索はこれだけを使い、ベクトル検索は [`visible_embedded`] を使う。
pub fn visible(types: usize, keys: usize, locale: usize) -> String {
    format!(
        "e.id = c.entry_id
         AND e.deleted_at IS NULL
         AND e.published_version_id = c.version_id
         AND e.use_for_chatbot
         AND e.locale = ${locale} AND c.locale = ${locale}
         AND (e.content_type, c.fields_key) IN (SELECT * FROM unnest(${types}::text[], ${keys}::text[]))
         AND EXISTS (SELECT 1 FROM versions v WHERE v.id = c.version_id AND v.erased_at IS NULL)"
    )
}

/// [`visible`] に加えて、チャンクのテキストの、今の埋め込みのモデルと次元数（`{model}`・`{dims}` のパラメーターの番号）の
/// 埋め込み。`x` は search_embeddings。
pub fn visible_embedded(
    types: usize,
    keys: usize,
    locale: usize,
    model: usize,
    dims: usize,
) -> String {
    format!(
        "{}
         AND x.content_hash = c.content_hash AND x.model = ${model} AND x.dimensions = ${dims}",
        visible(types, keys, locale)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_change_when_searchable_fields_change() {
        let a = Plan {
            fields: vec![("body".into(), true)],
            title: None,
        };
        let mut b = a.clone();
        assert_eq!(fields_key(&a), fields_key(&b));
        b.fields.push(("subtitle".into(), false));
        assert_ne!(fields_key(&a), fields_key(&b));
        let mut c = a.clone();
        c.title = Some("title".into());
        assert_ne!(fields_key(&a), fields_key(&c));
    }

    #[test]
    fn visible_uses_only_numbered_parameters() {
        let sql = visible_embedded(1, 2, 3, 4, 5);
        assert!(sql.contains("$1::text[]") && sql.contains("$2::text[]"));
        assert!(
            sql.contains("e.locale = $3")
                && sql.contains("x.model = $4")
                && sql.contains("x.dimensions = $5")
        );
        assert!(sql.contains("e.published_version_id = c.version_id"));
        assert!(sql.contains("e.use_for_chatbot"));
        assert!(sql.contains("x.content_hash = c.content_hash"));
        // キーワード検索の条件は、モデルによらない（同じ照合を含む）
        let text = visible(1, 2, 3);
        assert!(sql.starts_with(&text));
        assert!(!text.contains("x."));
    }
}
