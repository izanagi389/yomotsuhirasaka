//! 同梱のテンプレート（13-security.md「デプロイ」、2026-11-11）。
//!
//! リリースに同梱した見本のテンプレートを、サーバーのバイナリに埋め込む。中身は `templates/<名前>` を CLI の
//! `yomi schema build` でまとめた JSON（`server/bundled-templates/<名前>.json`）で、`scripts/check-bundled-templates.mjs`
//! が `templates/` から作り直した結果と一致することを確かめる。サーバー管理者は、これだけを管理画面からデプロイできる。
//!
//! 見本の公開URLのひな形は [`PLACEHOLDER_ORIGIN`] で始まる。デプロイのときに、サーバー管理者が入れたサイトのオリジンに
//! 置き換える（置き換えるのは、URLのひな形の先頭のオリジンだけ。ほかの中身と関数は変えない）。

use serde_json::Value;

use super::manifest::Manifest;

/// 同梱のテンプレートの名前と、まとめたマニフェストの JSON。
const BUNDLED: &[(&str, &str)] = &[("blog", include_str!("../../bundled-templates/blog.json"))];

/// 見本の公開URLのひな形の先頭のオリジン（デプロイのときに置き換える）。
pub const PLACEHOLDER_ORIGIN: &str = "https://example.com";

/// 同梱のテンプレートの名前。
pub fn names() -> impl Iterator<Item = &'static str> {
    BUNDLED.iter().map(|(name, _)| *name)
}

fn raw(name: &str) -> Option<Value> {
    let (_, json) = BUNDLED.iter().find(|(n, _)| *n == name)?;
    serde_json::from_str(json).ok()
}

/// 公開URLのひな形（`contentTypes[].urlTemplate`）の先頭のオリジンを置き換える。
fn replace_origin(mut v: Value, from: &str, to: &str) -> Value {
    if let Some(types) = v.get_mut("contentTypes").and_then(Value::as_array_mut) {
        for ct in types {
            if let Some(Value::String(t)) = ct.get_mut("urlTemplate")
                && let Some(rest) = t.strip_prefix(from)
                && (rest.is_empty() || rest.starts_with('/'))
            {
                *t = format!("{to}{rest}");
            }
        }
    }
    v
}

/// サイトのオリジン: `https://host[:port]`。手元で試すための `http://localhost`・`http://127.0.0.1` も受け付ける。
/// パス・クエリ・利用者の情報は受け付けない。正規化したオリジンを返す。
pub fn site_origin(input: &str) -> Option<String> {
    let u = url::Url::parse(input.trim()).ok()?;
    let host = u.host_str()?;
    let local = matches!(host, "localhost" | "127.0.0.1" | "[::1]");
    let scheme_ok = u.scheme() == "https" || (u.scheme() == "http" && local);
    let bare = matches!(u.path(), "" | "/")
        && u.query().is_none()
        && u.fragment().is_none()
        && u.username().is_empty()
        && u.password().is_none();
    (scheme_ok && bare).then(|| u.origin().ascii_serialization())
}

/// 同梱のテンプレートのマニフェストを、サイトのオリジンを差し込んで作る（なければ None）。
pub fn manifest_for(name: &str, origin: &str) -> Option<Manifest> {
    serde_json::from_value(replace_origin(raw(name)?, PLACEHOLDER_ORIGIN, origin)).ok()
}

/// デプロイ済みのマニフェストのサイトのオリジン（最初のコンテンツタイプの公開URLのひな形から）。
pub fn origin_of(deployed: &Manifest) -> Option<String> {
    let v = serde_json::to_value(deployed).ok()?;
    let t = v
        .get("contentTypes")?
        .as_array()?
        .first()?
        .get("urlTemplate")?
        .as_str()?;
    let u = url::Url::parse(t.split('{').next()?).ok()?;
    Some(u.origin().ascii_serialization())
}

/// デプロイ済みのマニフェストが、同梱の中身（サイトのオリジンを除く）と同じか。
pub fn same_as_bundled(name: &str, deployed: &Manifest) -> bool {
    let Some(origin) = origin_of(deployed) else {
        return false;
    };
    manifest_for(name, &origin).is_some_and(|m| &m == deployed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_template_is_a_valid_manifest() {
        for name in names() {
            assert!(super::super::manifest::valid_template_name(name), "{name}");
            let m = manifest_for(name, "https://blog.example.org")
                .unwrap_or_else(|| panic!("{name} does not parse"));
            let problems = super::super::manifest::validate(&m);
            assert!(problems.is_empty(), "{name}: {problems:?}");
        }
        assert!(manifest_for("no-such-template", "https://blog.example.org").is_none());
    }

    #[test]
    fn the_site_origin_replaces_only_the_url_template_origin() {
        let m = manifest_for("blog", "https://blog.example.org").unwrap();
        let v = serde_json::to_value(&m).unwrap();
        let t = v["contentTypes"][0]["urlTemplate"].as_str().unwrap();
        assert!(t.starts_with("https://blog.example.org/"), "{t}");
        assert!(
            !serde_json::to_string(&v)
                .unwrap()
                .contains(PLACEHOLDER_ORIGIN)
        );
        assert_eq!(origin_of(&m).as_deref(), Some("https://blog.example.org"));
        assert!(same_as_bundled("blog", &m));
        let other = manifest_for("blog", "https://other.example.org").unwrap();
        assert!(same_as_bundled("blog", &other));
    }

    #[test]
    fn site_origins_are_restricted() {
        assert_eq!(
            site_origin("https://Blog.Example.org/").as_deref(),
            Some("https://blog.example.org")
        );
        assert_eq!(
            site_origin("https://example.org:8443").as_deref(),
            Some("https://example.org:8443")
        );
        assert_eq!(
            site_origin("http://localhost:3000").as_deref(),
            Some("http://localhost:3000")
        );
        for bad in [
            "http://example.org",
            "https://example.org/blog",
            "https://example.org/?x=1",
            "https://user@example.org",
            "ftp://example.org",
            "example.org",
            "",
        ] {
            assert!(site_origin(bad).is_none(), "{bad}");
        }
    }
}
