//! 配信APIのGraphQLの名前（02-schema-and-delivery-api.md）。
//!
//! コンテンツタイプ `blog_post` から、次の名前を機械的に決める。
//! - 型: `BlogPost`
//! - 1件: `blogPost(id, locale)`、スラッグで1件: `blogPostBySlug(slug, locale)`、一覧: `blogPostList(locale, limit, offset)`
//!
//! 記事の型には、CMSが決めるフィールド（`id`・`locale`・`url`・`publishedAt`・`firstPublishedAt`・`translationGroupId`・
//! `alternates`・`corrections`・`deletedAnchors`・`lastVerifiedAt`・`reviewDueAt`・`reviewExpired`）を置く。テンプレートのフィールドは名前をそのまま使う（英小文字・数字・アンダースコア）。
//! 名前がぶつかるテンプレートは、デプロイの時点で止める（原則10）。

use std::collections::BTreeSet;

use crate::schema::manifest::Manifest;

/// CMSが使う型の名前。
pub const QUERY_TYPE: &str = "Query";
pub const JSON_SCALAR: &str = "CmsJSON";
pub const ALTERNATE_TYPE: &str = "CmsAlternate";
pub const REDIRECT_TYPE: &str = "CmsRedirect";
/// CMSが置くルートのフィールド（リダイレクト一覧）。コンテンツタイプから決まる名前とぶつけない。
pub const REDIRECTS_FIELD: &str = "redirects";
pub const CORRECTION_TYPE: &str = "CmsCorrection";
pub const DELETED_ANCHOR_TYPE: &str = "CmsDeletedAnchor";
pub const CORRECTION_FEED_ITEM_TYPE: &str = "CmsCorrectionFeedItem";
/// CMSが置くルートのフィールド（訂正フィード）。記事の型の訂正履歴のフィールドも同じ名前。
pub const CORRECTIONS_FIELD: &str = "corrections";

/// 記事の型のCMSのフィールドのうち、テンプレートのフィールド名（英小文字・数字・_）とぶつかりうるもの。
pub const RESERVED_FIELDS: [&str; 5] = ["id", "locale", "url", "alternates", CORRECTIONS_FIELD];

/// GraphQLの組み込みの型と、CMSが使う型の名前。
const RESERVED_TYPES: [&str; 10] = [
    QUERY_TYPE,
    "Mutation",
    "Subscription",
    "String",
    "Int",
    "Float",
    "Boolean",
    "ID",
    JSON_SCALAR,
    ALTERNATE_TYPE,
];

fn parts(name: &str) -> impl Iterator<Item = &str> {
    name.split('_').filter(|p| !p.is_empty())
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
        None => String::new(),
    }
}

/// 型の名前（`blog_post` → `BlogPost`）。
pub fn type_name(content_type: &str) -> String {
    parts(content_type).map(capitalize).collect()
}

/// 1件を返すフィールドの名前（`blog_post` → `blogPost`）。
pub fn single(content_type: &str) -> String {
    let mut it = parts(content_type);
    let first = it.next().unwrap_or_default().to_owned();
    first + &it.map(capitalize).collect::<String>()
}

pub fn by_slug(content_type: &str) -> String {
    format!("{}BySlug", single(content_type))
}

pub fn list(content_type: &str) -> String {
    format!("{}List", single(content_type))
}

/// 名前のぶつかりを調べる（マニフェストの検査から呼ぶ）。
pub fn problems(m: &Manifest) -> Vec<String> {
    let mut problems = Vec::new();
    let mut types = BTreeSet::new();
    let mut roots = BTreeSet::from([REDIRECTS_FIELD.to_owned(), CORRECTIONS_FIELD.to_owned()]);
    for (i, ct) in m.content_types.iter().enumerate() {
        let t = type_name(&ct.name);
        if RESERVED_TYPES.contains(&t.as_str()) || t.starts_with("Cms") || !types.insert(t) {
            problems.push(format!(
                "contentTypes[{i}].name: the GraphQL type name derived from it is reserved or used by another content type"
            ));
        }
        for root in [single(&ct.name), by_slug(&ct.name), list(&ct.name)] {
            if !roots.insert(root) {
                problems.push(format!(
                    "contentTypes[{i}].name: a GraphQL query name derived from it is used by another content type"
                ));
            }
        }
        for (j, f) in ct.fields.iter().enumerate() {
            if RESERVED_FIELDS.contains(&f.name.as_str()) {
                problems.push(format!(
                    "contentTypes[{i}].fields[{j}].name: `{}` is reserved for the field the CMS provides",
                    f.name
                ));
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(type_name("post"), "Post");
        assert_eq!(type_name("blog_post_2"), "BlogPost2");
        assert_eq!(single("blog_post"), "blogPost");
        assert_eq!(by_slug("post"), "postBySlug");
        assert_eq!(list("blog_post"), "blogPostList");
    }

    #[test]
    fn collisions_are_reported() {
        let mut m = crate::schema::manifest::tests::sample();
        assert!(problems(&m).is_empty());
        let mut other = m.content_types[0].clone();
        other.name = "post_list".into();
        m.content_types.push(other.clone());
        assert!(!problems(&m).is_empty(), "postList collides");
        m.content_types.pop();
        other.name = "redirects".into();
        m.content_types.push(other.clone());
        assert!(!problems(&m).is_empty(), "redirects is reserved");
        m.content_types.pop();
        other.name = "corrections".into();
        m.content_types.push(other.clone());
        assert!(!problems(&m).is_empty(), "corrections is reserved");
        m.content_types.pop();
        other.name = "query".into();
        m.content_types.push(other.clone());
        assert!(!problems(&m).is_empty(), "Query is reserved");
        m.content_types.pop();
        m.content_types[0].fields[0].name = "url".into();
        assert!(!problems(&m).is_empty(), "url is reserved");
    }
}
