//! 質問応答のプロンプトと回答の形（判定の先行宣言と `[n]` 形式の引用。09-qa-chatbot.md、Q15, Q27）。
//!
//! 形は `tools/mock-llm/src/format.json` の1か所に書き、擬似LLMとCMSの両方が読む（原則2）。
//! CMSは、宣言の行と引用記号を、このファイルの正規表現で読む。JSONモードなどには頼らない（Q7）。

use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;

pub(crate) const FORMAT_JSON: &str = include_str!("../../../tools/mock-llm/src/format.json");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Raw {
    fragment_line: String,
    declaration_line: String,
    answerable_declaration: String,
    not_answerable_declaration: String,
    source_separator: String,
    citation: String,
}

pub struct Format {
    /// プロンプトの断片の行（`[n] 本文`）。CMSが書く行がこの形に合うことを、テストで確かめる。
    pub fragment_line: Regex,
    /// 判定の先行宣言の行。1つ目の組が `yes`・`no`、2つ目が `none` か番号の並び。
    pub declaration_line: Regex,
    /// 「答えられる」の宣言のひな形（`{sources}` に番号の並び）。
    pub answerable_declaration: String,
    pub not_answerable_declaration: String,
    pub source_separator: String,
    /// 本文の中の引用記号。1つ目の組が番号。
    pub citation: Regex,
}

pub static FORMAT: LazyLock<Format> = LazyLock::new(|| {
    let raw: Raw = serde_json::from_str(FORMAT_JSON).expect("format.json is valid");
    let re = |p: &str| Regex::new(p).expect("format.json regex is valid");
    Format {
        fragment_line: re(&raw.fragment_line),
        declaration_line: re(&raw.declaration_line),
        answerable_declaration: raw.answerable_declaration,
        not_answerable_declaration: raw.not_answerable_declaration,
        source_separator: raw.source_separator,
        citation: re(&raw.citation),
    }
});

/// 宣言の行の解釈。番号は書かれた順（重なりは1つにする）。形に合わなければ None。
/// `Some(vec![])` は「答えられない」。
pub fn parse_declaration(line: &str) -> Option<Vec<usize>> {
    let caps = FORMAT.declaration_line.captures(line.trim())?;
    let answerable = caps.get(1)?.as_str() == "yes";
    let sources = caps.get(2)?.as_str();
    if !answerable {
        return (sources == "none").then(Vec::new);
    }
    if sources == "none" {
        return None;
    }
    let mut out: Vec<usize> = Vec::new();
    for part in sources.split(FORMAT.source_separator.as_str()) {
        // 正規表現の \d は Unicode の数字にも合うので、ASCII の数字だけを受け付ける
        if !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let n: usize = part.parse().ok()?;
        if !out.contains(&n) {
            out.push(n);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// プロンプトの断片の1行（番号は1から）。改行は空白にする（1つの断片を1行に収める）。
pub fn fragment_line(n: usize, text: &str) -> String {
    let one_line: String = text
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    format!("[{n}] {}", one_line.trim())
}

/// 本文の中の引用記号の番号（現れた順。ASCII の数字だけ）。
pub fn citations(text: &str) -> Vec<(std::ops::Range<usize>, Option<usize>)> {
    FORMAT
        .citation
        .captures_iter(text)
        .filter_map(|c| {
            let whole = c.get(0)?;
            let digits = c.get(1)?.as_str();
            let n = digits
                .bytes()
                .all(|b| b.is_ascii_digit())
                .then(|| digits.parse().ok())
                .flatten();
            Some((whole.range(), n))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declarations() {
        assert_eq!(
            parse_declaration("ANSWERABLE: yes; SOURCES: 1,3"),
            Some(vec![1, 3])
        );
        assert_eq!(
            parse_declaration("ANSWERABLE: yes; SOURCES: 2,2"),
            Some(vec![2])
        );
        assert_eq!(
            parse_declaration("ANSWERABLE: no; SOURCES: none"),
            Some(vec![])
        );
        for bad in [
            "ANSWERABLE: yes; SOURCES: none",
            "ANSWERABLE: no; SOURCES: 1",
            "answerable: yes; sources: 1",
            "ANSWERABLE: yes; SOURCES: ١",
            "Sure! ANSWERABLE: yes; SOURCES: 1",
            "",
        ] {
            assert_eq!(parse_declaration(bad), None, "{bad}");
        }
    }

    #[test]
    fn templates_round_trip() {
        let yes = FORMAT.answerable_declaration.replace(
            "{sources}",
            &["1", "2"].join(FORMAT.source_separator.as_str()),
        );
        assert_eq!(parse_declaration(&yes), Some(vec![1, 2]));
        assert_eq!(
            parse_declaration(&FORMAT.not_answerable_declaration),
            Some(vec![])
        );
        let line = fragment_line(4, "見出し\n本文");
        let caps = FORMAT.fragment_line.captures(&line).unwrap();
        assert_eq!((&caps[1], &caps[2]), ("4", "見出し 本文"));
    }

    #[test]
    fn finds_citations() {
        let c = citations("A [1] B [12] C [x] [٣]");
        let numbers: Vec<Option<usize>> = c.iter().map(|(_, n)| *n).collect();
        assert_eq!(numbers, vec![Some(1), Some(12), None]);
    }
}
