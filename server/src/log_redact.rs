//! ログの伏せ字（13-security.md「ログ」、第三者の確認の R37）。
//!
//! 秘密や本文をログに書かないのが第一の決まりで、ここは書いてしまったときの最後の守り。
//! - 決まった名前のフィールド（`password`、`token`、`cookie`、`question` など）の値は出さない。
//! - どのフィールドでも、既知の形（CMSのキーとトークン、`Bearer`、接続文字列のパスワード、セッションのCookie）を伏せる。
//!   ログの文の中の自由な文（記事の本文や質問文）は見分けられないので、それらは書かない決まりで守る。

use std::fmt;
use std::sync::LazyLock;

use regex::Regex;
use tracing_subscriber::fmt::format::Writer;

const REDACTED: &str = "[redacted]";

/// 値を出さないフィールドの名前（小文字。この語を含む名前に効く）。
const SECRET_FIELDS: [&str; 10] = [
    "password",
    "passwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "authorization",
    "cookie",
    "question",
    "credential",
];
/// 名前が完全に一致するときだけ値を出さないフィールド。
const CONTENT_FIELDS: [&str; 3] = ["body", "content", "fields"];

static KNOWN_FORMS: LazyLock<[(Regex, &'static str); 4]> = LazyLock::new(|| {
    let re = |p: &str| Regex::new(p).expect("the pattern is valid");
    [
        (
            re(r"(yomi_(?:pat|key|prv)_|whsec_)[A-Za-z0-9_-]+"),
            "${1}[redacted]",
        ),
        (re(r"(?i)(bearer\s+)[A-Za-z0-9._~+/=-]+"), "${1}[redacted]"),
        (re(r"(://[^/\s:@]*:)[^@\s/]+@"), "${1}[redacted]@"),
        (re(r"(__Host-yomi_[a-z_]+=)[^;\s]+"), "${1}[redacted]"),
    ]
});

fn is_secret_field(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SECRET_FIELDS.iter().any(|s| lower.contains(s)) || CONTENT_FIELDS.contains(&lower.as_str())
}

/// 文の中の既知の形を伏せる。
pub fn redact_text(text: &str) -> String {
    let mut out = text.to_owned();
    for (pattern, replacement) in KNOWN_FORMS.iter() {
        if pattern.is_match(&out) {
            out = pattern.replace_all(&out, *replacement).into_owned();
        }
    }
    out
}

/// ログの1フィールドを書く（`fmt::format::debug_fn` に渡す）。
pub fn write_field(
    w: &mut Writer<'_>,
    field: &tracing::field::Field,
    value: &dyn fmt::Debug,
) -> fmt::Result {
    let name = field.name();
    if is_secret_field(name) {
        return write!(w, "{name}={REDACTED}");
    }
    let text = redact_text(&format!("{value:?}"));
    if name == "message" {
        write!(w, "{text}")
    } else {
        write!(w, "{name}={text}")
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::field::MakeExt as _;
    use tracing_subscriber::fmt::format::debug_fn;

    use super::*;

    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn known_forms_are_masked() {
        for (input, kept, gone) in [
            (
                "key yomi_key_abc-DEF_1 used",
                "yomi_key_[redacted]",
                "abc-DEF_1",
            ),
            ("yomi_pat_SECRET1", "yomi_pat_[redacted]", "SECRET1"),
            ("yomi_prv_SECRET2", "yomi_prv_[redacted]", "SECRET2"),
            ("whsec_SECRET3", "whsec_[redacted]", "SECRET3"),
            (
                "Authorization: Bearer abc.def",
                "Bearer [redacted]",
                "abc.def",
            ),
            (
                "postgres://yomi:hunter2@db:5432/yomi",
                "postgres://yomi:[redacted]@db",
                "hunter2",
            ),
            (
                "__Host-yomi_session=SECRET4; Path=/",
                "__Host-yomi_session=[redacted]",
                "SECRET4",
            ),
        ] {
            let out = redact_text(input);
            assert!(out.contains(kept), "{out}");
            assert!(!out.contains(gone), "{out}");
        }
        let plain = "https://example.com:8443/path 10:20:30 job failed";
        assert_eq!(redact_text(plain), plain);
    }

    #[test]
    fn secret_fields_and_known_forms_do_not_reach_the_log() {
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .fmt_fields(debug_fn(write_field).delimited(" "))
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(
                password = "PW-VALUE",
                session_token = "TOKEN-VALUE",
                question = "QUESTION-VALUE",
                body = "BODY-VALUE",
                job_id = 7,
                "sending yomi_key_MESSAGE-VALUE"
            );
        });
        let out = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        for gone in [
            "PW-VALUE",
            "TOKEN-VALUE",
            "QUESTION-VALUE",
            "BODY-VALUE",
            "MESSAGE-VALUE",
        ] {
            assert!(!out.contains(gone), "{out}");
        }
        assert!(out.contains("job_id=7"), "{out}");
        assert!(out.contains("sending yomi_key_[redacted]"), "{out}");
    }
}
