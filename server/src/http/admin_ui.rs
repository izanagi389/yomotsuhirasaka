//! 管理画面（Nuxt の SPA）の配信（01-architecture.md「管理画面」、13-security.md「管理画面の応答のセキュリティヘッダー」、Q114）。
//!
//! - `/admin/` の下で、設定の `[admin] dist_dir`（`nuxt generate` の出力）のファイルを返す。
//!   ファイルがないパスには `index.html` を返す（SPA の画面の切り替えはブラウザの中で行う）。
//! - CSP は `script-src 'self'` と、`index.html` の中のインラインのスクリプトのハッシュだけを許す。
//!   `unsafe-inline` と `unsafe-eval` は使わない。ハッシュは起動時に `index.html` から計算する。
//! - 利用者が指定したパスでも、`dist_dir` の外のファイルは返さない（実体のパスで比べる）。

use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result};
use axum::body::Body;
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use base64::Engine as _;
use sha2::{Digest as _, Sha256};

use super::AppState;
use super::error::ApiError;

/// 読み込んだ管理画面。
pub struct AdminUi {
    root: PathBuf,
    index_html: Vec<u8>,
    csp: HeaderValue,
}

/// `index.html` の中の、CSP の script-src の対象になるインラインのスクリプトの中身
/// （`src` のないもの。import map も対象。JSON などのデータは除く）。
fn inline_scripts(html: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let lower = html.to_ascii_lowercase();
    let mut at = 0;
    while let Some(start) = lower[at..].find("<script") {
        let open = at + start;
        let Some(tag_end) = lower[open..].find('>').map(|i| open + i) else {
            break;
        };
        let attrs = &lower[open + "<script".len()..tag_end];
        let Some(close) = lower[tag_end..].find("</script").map(|i| tag_end + i) else {
            break;
        };
        let executable = !attrs.contains("src=")
            && !["application/json", "application/ld+json"].iter().any(|t| {
                attrs.contains(&format!("type=\"{t}\"")) || attrs.contains(&format!("type={t}"))
            });
        if executable {
            out.push(&html[tag_end + 1..close]);
        }
        at = close;
    }
    out
}

/// 管理画面の CSP。メディアを別のドメインから配信するときは、そのオリジンの画像も許す。
fn csp_for(scripts: &[&str], media_origin: Option<&str>) -> String {
    let mut script_src = String::from("'self'");
    for s in scripts {
        let digest = Sha256::digest(s.as_bytes());
        script_src.push_str(&format!(
            " 'sha256-{}'",
            base64::engine::general_purpose::STANDARD.encode(digest)
        ));
    }
    let mut img_src = String::from("'self' data: blob:");
    if let Some(origin) = media_origin {
        img_src.push(' ');
        img_src.push_str(origin);
    }
    format!(
        "default-src 'self'; script-src {script_src}; style-src 'self'; img-src {img_src}; font-src 'self'; \
         connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'"
    )
}

/// URLのオリジン（`https://host[:port]`）。
fn origin(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    matches!(u.scheme(), "https" | "http").then(|| u.origin().ascii_serialization())
}

impl AdminUi {
    pub fn load(dir: &Path, media_public_base_url: Option<&str>) -> Result<Self> {
        let root = dir
            .canonicalize()
            .with_context(|| format!("admin UI directory {} does not exist", dir.display()))?;
        let index_html = std::fs::read(root.join("index.html"))
            .with_context(|| format!("admin UI {} has no index.html", root.display()))?;
        let text = std::str::from_utf8(&index_html).context("admin UI index.html is not UTF-8")?;
        let csp = csp_for(
            &inline_scripts(text),
            media_public_base_url.and_then(origin).as_deref(),
        );
        Ok(Self {
            root,
            index_html,
            csp: HeaderValue::from_str(&csp).context("admin UI CSP")?,
        })
    }

    /// パスに当たるファイル（`root` の中にあるものだけ）。
    fn file(&self, rel: &str) -> Option<PathBuf> {
        let rel = Path::new(rel);
        if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            return None;
        }
        let real = self.root.join(rel).canonicalize().ok()?;
        (real.starts_with(&self.root) && real.is_file()).then_some(real)
    }

    fn index(&self) -> Response {
        let mut res = (
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            self.index_html.clone(),
        )
            .into_response();
        let h = res.headers_mut();
        h.insert(header::CONTENT_SECURITY_POLICY, self.csp.clone());
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        res
    }
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// `/admin` → `/admin/`。
pub async fn root_redirect() -> Redirect {
    Redirect::permanent("/admin/")
}

pub async fn index(State(state): State<AppState>) -> Result<Response, ApiError> {
    let ui = state.admin_ui.as_ref().ok_or_else(ApiError::not_found)?;
    Ok(ui.index())
}

pub async fn serve(
    State(state): State<AppState>,
    UrlPath(rel): UrlPath<String>,
) -> Result<Response, ApiError> {
    let ui = state.admin_ui.as_ref().ok_or_else(ApiError::not_found)?;
    if let Some(path) = ui.file(&rel)
        && !path.ends_with("index.html")
    {
        let bytes = tokio::fs::read(&path)
            .await
            .with_context(|| format!("cannot read {}", path.display()))?;
        let mut res = (
            [(header::CONTENT_TYPE, content_type(&path))],
            Body::from(bytes),
        )
            .into_response();
        let h = res.headers_mut();
        // ビルドのたびに名前が変わるファイル（_nuxt/ の下）は長く持たせる。それ以外は毎回確かめさせる
        let cache = if rel.starts_with("_nuxt/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
        h.insert(header::CONTENT_SECURITY_POLICY, ui.csp.clone());
        return Ok(res);
    }
    // 拡張子のあるパス（ないファイル）は 404。それ以外は画面のパスとして index.html を返す
    let last = rel.rsplit('/').next().unwrap_or("");
    if last.contains('.') || rel.starts_with("_nuxt/") {
        return Ok((StatusCode::NOT_FOUND, "not found").into_response());
    }
    Ok(ui.index())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_only_executable_inline_scripts() {
        let html = r#"<html><head><script type="importmap">{"imports":{}}</script><script type="module" src="/admin/_nuxt/a.js"></script>
<script>window.__NUXT__={};</script><script type="application/json" data-nuxt-data>{"a":1}</script>
<SCRIPT>console.log(1)</SCRIPT></head></html>"#;
        assert_eq!(
            inline_scripts(html),
            vec![r#"{"imports":{}}"#, "window.__NUXT__={};", "console.log(1)"]
        );
        let csp = csp_for(&inline_scripts(html), Some("https://media.example.com"));
        assert!(csp.contains("script-src 'self' 'sha256-"));
        assert_eq!(csp.matches("'sha256-").count(), 3);
        assert!(!csp.contains("unsafe"));
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(csp.contains("img-src 'self' data: blob: https://media.example.com"));
        // 期待値: echo -n 'window.__NUXT__={};' | openssl dgst -sha256 -binary | base64
        let d = Sha256::digest(b"window.__NUXT__={};");
        assert!(csp.contains(&base64::engine::general_purpose::STANDARD.encode(d)));
    }

    #[test]
    fn files_outside_the_root_are_not_served() {
        let dir =
            std::env::temp_dir().join(format!("cms-admin-ui-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(dir.join("_nuxt")).unwrap();
        std::fs::write(dir.join("index.html"), "<html><script>x()</script></html>").unwrap();
        std::fs::write(dir.join("_nuxt/app.js"), "x").unwrap();
        std::fs::write(dir.with_extension("secret"), "s").unwrap();
        let ui = AdminUi::load(&dir, None).unwrap();
        assert!(ui.file("_nuxt/app.js").is_some());
        for bad in ["../x.secret", "_nuxt/../../x.secret", "/etc/passwd"] {
            assert!(ui.file(bad).is_none(), "{bad}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
