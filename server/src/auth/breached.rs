//! 漏えいしたパスワードの照会（Have I Been Pwned の range API。13-security.md「パスワードの強さ」、Q170）。
//!
//! - 送るのは、パスワードの SHA-1 の16進の先頭5文字だけ（k-匿名性）。残りの35文字は、返ってきた一覧と手元で比べる。
//! - 照会は外部への送信の共通の仕組み（egress）を通す。`Add-Padding` で、応答の大きさからも推測されにくくする。
//! - 照会先に届かない・時間がかかる・応答が読めないときは「確かめられなかった」として、登録を止めない。
//! - 既定では無効。サーバー管理者が設定で有効にする。

use std::time::Duration;

use sha1::{Digest as _, Sha1};

use crate::config::BreachedPasswordsConfig;
use crate::egress::{Egress, EgressRequest};

/// 応答の本文の上限（HIBP の1つの範囲は、詰め物を入れても数十KB）。
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// 照会の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 漏えいの一覧に載っている。
    Breached,
    /// 載っていない。
    NotFound,
    /// 確かめられなかった（照会先に届かないなど）。登録は止めない。
    Unknown,
}

fn sha1_hex_upper(password: &str) -> String {
    use std::fmt::Write as _;
    Sha1::digest(password.as_bytes())
        .iter()
        .fold(String::with_capacity(40), |mut s, b| {
            let _ = write!(s, "{b:02X}");
            s
        })
}

/// 応答（`SUFFIX:COUNT` の行）に、ハッシュの残りの35文字が、1回以上として載っているか。
fn listed(body: &str, suffix: &str) -> bool {
    body.lines().any(|line| {
        let mut parts = line.trim().splitn(2, ':');
        let s = parts.next().unwrap_or("");
        let count = parts
            .next()
            .and_then(|c| c.trim().parse::<u64>().ok())
            .unwrap_or(0);
        // 詰め物の行は回数が0
        count > 0 && s.eq_ignore_ascii_case(suffix)
    })
}

/// 照会する。
pub async fn check(
    egress: Option<&Egress>,
    config: &BreachedPasswordsConfig,
    password: &str,
) -> Verdict {
    let Some(egress) = egress else {
        return Verdict::Unknown;
    };
    let hash = sha1_hex_upper(password);
    let (prefix, suffix) = hash.split_at(5);
    let url = format!("{}/range/{prefix}", config.base_url.trim_end_matches('/'));
    let request = EgressRequest {
        method: hyper::Method::GET,
        url,
        headers: vec![
            (
                hyper::header::HeaderName::from_static("add-padding"),
                hyper::header::HeaderValue::from_static("true"),
            ),
            (
                hyper::header::USER_AGENT,
                hyper::header::HeaderValue::from_static("cms-password-check/1"),
            ),
        ],
        body: Vec::new(),
        read_body_limit: Some(MAX_RESPONSE_BYTES),
    };
    let timeout = Duration::from_secs(config.timeout_seconds.max(1).into());
    match tokio::time::timeout(timeout, egress.send(request)).await {
        Ok(Ok(res)) if res.status == 200 => match res.body.as_deref().map(std::str::from_utf8) {
            Some(Ok(body)) => {
                if listed(body, suffix) {
                    Verdict::Breached
                } else {
                    Verdict::NotFound
                }
            }
            _ => Verdict::Unknown,
        },
        Ok(Ok(res)) => {
            tracing::warn!(
                status = res.status,
                "breached password check: unexpected status; skipped"
            );
            Verdict::Unknown
        }
        Ok(Err(e)) => {
            tracing::warn!("breached password check failed ({e}); skipped");
            Verdict::Unknown
        }
        Err(_) => {
            tracing::warn!("breached password check timed out; skipped");
            Verdict::Unknown
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_matching() {
        // "password" の SHA-1 は 5BAA61E4C9B93F3F0682250B6CF8331B7EE68FD8
        assert_eq!(
            sha1_hex_upper("password"),
            "5BAA61E4C9B93F3F0682250B6CF8331B7EE68FD8"
        );
        let suffix = "1E4C9B93F3F0682250B6CF8331B7EE68FD8";
        assert!(listed(
            "0018A45C4D1DEF81644B54AB7F969B88D65:1\r\n1e4c9b93f3f0682250b6cf8331b7ee68fd8:3730471",
            suffix
        ));
        assert!(!listed("0018A45C4D1DEF81644B54AB7F969B88D65:1", suffix));
        // 詰め物（回数0）の行は数えない
        assert!(!listed(&format!("{suffix}:0"), suffix));
    }
}
