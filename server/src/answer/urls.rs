//! 回答の中のURLの絞り込み（13-security.md「チャットボットへの悪用」、INV-QA-11）。
//!
//! 回答の本文（Markdown）に残してよいURLは、http/https のURLのうち、次のどちらかだけ。
//! - 根拠にした断片（LLMに渡した文字列）に、そのまま書かれているURL
//! - 根拠にした記事の公開URLと、オリジンが同じURL
//!
//! 見るのは、リンク `[文](URL)`、画像 `![代替](URL)`、リンクの参照の定義 `[名]: URL`、自動リンク `<URL>`、
//! そのままのURL（`http://`・`https://`・`www.` で始まる語。GFM の自動リンクになるもの）。
//! - 許さないリンクは、文だけを残す。許さない画像は、代替の文ごと取り除く。
//! - 許さない定義・自動リンク・そのままのURLは、取り除く。
//! - 生の HTML はエスケープして表示する仕様（公式のSDK）なので、タグとしては扱わない。中の http(s) のURLは、そのままの
//!   URLとして絞り込む。
//!
//! ストリーミングでは、`[`・`![`・`<`・`h`・`w` から始まる部分を、形が決まるまで（[`HOLD_CHARS`] 文字まで）配信せずに待つ。
//! 形が決まらないまま長くなったら、リンクではない文字として扱う（中のURLは、そのままのURLとして絞り込む）。
//!
//! オリジンで許すのは、表示する側と解釈が分かれうる文字（`\`・`&`・空白・制御文字）を含まないURLだけ。
//! Markdown はリンク先の `\` のエスケープと文字参照を解いてから使うため、そのまま解釈すると別のホストに見えうる。

use std::collections::HashSet;

use url::Url;

/// 形が決まるまで配信せずに待つ長さ（文字）。
pub const HOLD_CHARS: usize = 500;

/// 断片1つから分かる、許すURLの手がかり。
#[derive(Debug, Clone, Default)]
pub struct Ground {
    /// 断片に書かれたURL（書かれたとおりの文字列）。
    urls: Vec<String>,
    /// 記事の公開URLのオリジン。
    origin: Option<String>,
}

impl Ground {
    /// `text` はLLMに渡した断片の文字列、`public_url` は記事の公開URL。
    pub fn new(text: &str, public_url: Option<&str>) -> Self {
        Self {
            urls: urls_in(text),
            origin: public_url.and_then(origin),
        }
    }
}

/// http/https のURLのオリジン。
fn origin(url: &str) -> Option<String> {
    let u = Url::parse(url).ok()?;
    matches!(u.scheme(), "http" | "https").then(|| u.origin().ascii_serialization())
}

/// 許すURL（宣言した出典の断片から作る）。
#[derive(Debug, Default)]
pub struct Allowed {
    literal: HashSet<String>,
    origins: HashSet<String>,
}

impl Allowed {
    pub fn new<'a>(grounds: impl IntoIterator<Item = &'a Ground>) -> Self {
        let mut out = Self::default();
        for g in grounds {
            out.literal.extend(g.urls.iter().cloned());
            out.origins.extend(g.origin.iter().cloned());
        }
        out
    }

    /// `url` を残してよいか。`www` は `www.` で始まるそのままのURL（スキームは http とみなす）。
    fn permits(&self, url: &str, www: bool) -> bool {
        let full = if www {
            format!("http://{url}")
        } else {
            url.to_owned()
        };
        let Ok(parsed) = Url::parse(&full) else {
            return false;
        };
        if !matches!(parsed.scheme(), "http" | "https") {
            return false;
        }
        if self.literal.contains(url) {
            return true;
        }
        if url.contains(['\\', '&']) || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return false;
        }
        self.origins
            .contains(&parsed.origin().ascii_serialization())
    }
}

/// 本文に書かれたURL（リンク・画像・定義・自動リンク・そのままのURL。http/https 以外も含む）。
pub fn urls_in(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut out = String::new();
    scan(
        text,
        true,
        &mut State::default(),
        &mut |u, _| {
            found.push(u.to_owned());
            true
        },
        &mut out,
    );
    found
}

/// 届いた順に絞り込む。
#[derive(Debug)]
pub struct Filter {
    allowed: Allowed,
    pending: String,
    state: State,
}

impl Filter {
    pub fn new(allowed: Allowed) -> Self {
        Self {
            allowed,
            pending: String::new(),
            state: State::default(),
        }
    }

    /// 断片を1つ読み、配信してよい文字列を返す（形が決まらない部分は持っておく）。
    pub fn push(&mut self, text: &str) -> String {
        self.pending.push_str(text);
        self.run(false)
    }

    /// 終わり。持っていた部分を、終わりとして解釈して返す。
    pub fn finish(&mut self) -> String {
        self.run(true)
    }

    fn run(&mut self, end: bool) -> String {
        let mut out = String::new();
        let allowed = &self.allowed;
        let used = scan(
            &self.pending,
            end,
            &mut self.state,
            &mut |u, www| allowed.permits(u, www),
            &mut out,
        );
        self.pending.drain(..used);
        out
    }
}

/// 読み進めた位置の状態（断片の境目をまたいで持つ）。
#[derive(Debug, Clone)]
struct State {
    /// 直前に出した文字。
    prev: Option<char>,
    /// 長すぎるそのままのURLを、空白まで捨てている途中か。
    skipping: bool,
    /// 行の頭から、空白とリスト・引用の記号しか出していないか（参照の定義は行の頭にだけ書ける）。
    line_start: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            prev: None,
            skipping: false,
            line_start: true,
        }
    }
}

impl State {
    fn emitted(&mut self, s: &str) {
        for c in s.chars() {
            // CommonMark の行の終わりは `\n`・`\r\n`・`\r`
            if c == '\n' || c == '\r' {
                self.line_start = true;
            } else if !matches!(
                c,
                ' ' | '\t' | '>' | '-' | '*' | '+' | '.' | ')' | '0'..='9'
            ) {
                self.line_start = false;
            }
            self.prev = Some(c);
        }
    }
}

enum Parsed {
    /// `len` バイトを読み、代わりに `out` を出す。
    Done(usize, String),
    /// 続きが届かないと決まらない。
    More,
    /// その形ではない（1文字をそのまま出す）。
    No,
}

type Policy<'a> = dyn FnMut(&str, bool) -> bool + 'a;

/// `s` を読み、絞り込んだ文字列を `out` に足す。読み終えたバイト数を返す（残りは、続きを待つ部分）。
fn scan(s: &str, end: bool, st: &mut State, policy: &mut Policy<'_>, out: &mut String) -> usize {
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        let c = rest.chars().next().expect("not empty");
        if st.skipping {
            if c.is_ascii_whitespace() || c == '<' {
                st.skipping = false;
            } else {
                i += c.len_utf8();
                continue;
            }
        }
        let parsed = match c {
            // どの形にも当たらない `]` の直後が `(` なら、エスケープして出す。こちらの読み方（コードスパンなど）と
            // 表示する側の解釈がずれても、`](` でリンクや画像にならないようにする
            ']' => match rest[1..].chars().next() {
                None if !end => Parsed::More,
                Some('(') => Parsed::Done(1, "\\]".to_owned()),
                _ => Parsed::No,
            },
            '\\' => match rest[1..].chars().next() {
                None if !end => Parsed::More,
                Some(n) if n.is_ascii_punctuation() => {
                    let len = 1 + n.len_utf8();
                    Parsed::Done(len, rest[..len].to_owned())
                }
                _ => Parsed::No,
            },
            '!' => match rest[1..].chars().next() {
                None if !end => Parsed::More,
                Some('[') => link(rest, true, false, end, policy),
                _ => Parsed::No,
            },
            '[' => link(rest, false, st.line_start, end, policy),
            '<' => autolink(rest, end, policy),
            'h' | 'H' => bare(rest, end, policy),
            'w' | 'W' if !st.prev.is_some_and(|p| p.is_ascii_alphanumeric()) => {
                bare(rest, end, policy)
            }
            _ => Parsed::No,
        };
        // 待てる長さを超える形は、届き方によらず同じに扱うため、長すぎるものとして扱う
        let parsed = match parsed {
            Parsed::Done(len, _) if c != '\\' && rest[..len].chars().count() > HOLD_CHARS => {
                Parsed::More
            }
            p => p,
        };
        match parsed {
            Parsed::Done(len, text) => {
                out.push_str(&text);
                st.emitted(&text);
                i += len;
            }
            Parsed::More if !end && rest.chars().count() <= HOLD_CHARS => return i,
            // 長すぎるそのままのURLは、空白まで捨てる
            Parsed::More if matches!(c, 'h' | 'H' | 'w' | 'W') => {
                st.skipping = true;
            }
            // 長すぎて形を決められない `[`・`<` は、エスケープして出す（後から届く文と合わせて、リンクや自動リンクにならない）
            Parsed::More if matches!(c, '[' | '<') => {
                let escaped = format!("\\{c}");
                out.push_str(&escaped);
                st.emitted(&escaped);
                i += 1;
            }
            Parsed::More | Parsed::No => {
                out.push(c);
                st.emitted(&rest[..c.len_utf8()]);
                i += c.len_utf8();
            }
        }
    }
    s.len()
}

/// 全体がそろった文字列を絞り込む（リンクの文の中など）。
fn filter_all(s: &str, policy: &mut Policy<'_>) -> String {
    let mut out = String::new();
    scan(s, true, &mut State::default(), policy, &mut out);
    out
}

fn need(end: bool) -> Parsed {
    if end { Parsed::No } else { Parsed::More }
}

fn skip_ws(b: &[u8], mut k: usize) -> usize {
    while k < b.len() && matches!(b[k], b' ' | b'\t' | b'\n' | b'\r') {
        k += 1;
    }
    k
}

/// リンクの宛先（`<...>` か、かっこの釣り合った空白のない並び）。（宛先の後の位置, 中身の始まり, 中身の終わり）。
/// 続きが要れば Err(true)、宛先の形でなければ Err(false)。
fn destination(b: &[u8], k: usize, parens: bool, end: bool) -> Result<(usize, usize, usize), bool> {
    if k >= b.len() {
        return Err(true);
    }
    if b[k] == b'<' {
        let mut m = k + 1;
        while m < b.len() {
            match b[m] {
                b'\\' => m += 2,
                b'>' => return Ok((m + 1, k + 1, m)),
                b'\n' | b'<' => return Err(false),
                _ => m += 1,
            }
        }
        return Err(true);
    }
    let mut depth = 0usize;
    let mut m = k;
    while m < b.len() {
        let c = b[m];
        if c == b'\\' {
            m += 2;
            continue;
        }
        if c.is_ascii_whitespace() || c.is_ascii_control() {
            break;
        }
        if parens && c == b'(' {
            depth += 1;
        }
        if parens && c == b')' {
            if depth == 0 {
                break;
            }
            depth -= 1;
        }
        m += 1;
    }
    if m >= b.len() {
        if !end {
            return Err(true);
        }
        m = b.len();
    }
    Ok((m, k, m))
}

/// `[文](宛先 "題")`・`![代替](宛先)`・`[名]: 宛先`。`rest` は `[` か `![` から。
fn link(rest: &str, image: bool, line_start: bool, end: bool, policy: &mut Policy<'_>) -> Parsed {
    let b = rest.as_bytes();
    let open = if image { 2 } else { 1 };
    let mut depth = 0usize;
    let mut j = open;
    let mut close = None;
    while j < b.len() {
        match b[j] {
            b'\\' => {
                j += 2;
                continue;
            }
            b'[' => depth += 1,
            b']' if depth == 0 => {
                close = Some(j);
                break;
            }
            b']' => depth -= 1,
            // 空の行をまたぐリンクはない
            b'\n' if b.get(j + 1) == Some(&b'\n') => return Parsed::No,
            _ => {}
        }
        j += 1;
    }
    let Some(close) = close else {
        return need(end);
    };
    let text = &rest[open..close];
    match b.get(close + 1) {
        None => need(end),
        Some(b'(') => {
            let k = skip_ws(b, close + 2);
            let (after, ds, de) = match destination(b, k, true, end) {
                Ok(d) => d,
                Err(more) => return if more { need(end) } else { Parsed::No },
            };
            let mut k = skip_ws(b, after);
            if k >= b.len() {
                return need(end);
            }
            if matches!(b[k], b'"' | b'\'' | b'(') {
                let Some(t) = title_end(b, k) else {
                    return need(end);
                };
                k = skip_ws(b, t);
                if k >= b.len() {
                    return need(end);
                }
            }
            if b[k] != b')' {
                return Parsed::No;
            }
            let dest = &rest[ds..de];
            // 題は出さない（題の中のURLを、リンクの外の文として解釈されないように）
            let written = &rest[if ds > 0 && b[ds - 1] == b'<' {
                ds - 1
            } else {
                ds
            }..after];
            let out = match (policy(dest, false), image) {
                (true, true) => format!("![{text}]({written})"),
                (true, false) => format!("[{}]({written})", filter_all(text, policy)),
                (false, true) => String::new(),
                (false, false) => filter_all(text, policy),
            };
            Parsed::Done(k + 1, out)
        }
        Some(b':') if !image && line_start => {
            let k = skip_ws(b, close + 2);
            let (after, ds, de) = match destination(b, k, false, end) {
                Ok(d) => d,
                Err(more) => return if more { need(end) } else { Parsed::No },
            };
            // 宛先の後は、行の終わりか、題と行の終わりでなければ定義ではない（CommonMark）
            let mut m = after;
            while m < b.len() && matches!(b[m], b' ' | b'\t') {
                m += 1;
            }
            if m < b.len() && !matches!(b[m], b'\n' | b'\r') {
                if !matches!(b[m], b'"' | b'\'' | b'(') {
                    return Parsed::No;
                }
                let Some(t) = title_end(b, m) else {
                    return need(end);
                };
                m = t;
                while m < b.len() && matches!(b[m], b' ' | b'\t') {
                    m += 1;
                }
                if m < b.len() && !matches!(b[m], b'\n' | b'\r') {
                    return Parsed::No;
                }
            }
            if m >= b.len() && !end {
                return Parsed::More;
            }
            let out = if policy(&rest[ds..de], false) {
                rest[..after].to_owned()
            } else {
                String::new()
            };
            Parsed::Done(after, out)
        }
        _ => Parsed::No,
    }
}

/// 題（`"..."`・`'...'`・`(...)`。`k` はその開き）の後の位置。閉じがまだ届いていなければ None。
fn title_end(b: &[u8], k: usize) -> Option<usize> {
    let closing = if b[k] == b'(' { b')' } else { b[k] };
    let mut m = k + 1;
    while m < b.len() {
        if b[m] == b'\\' {
            m += 2;
            continue;
        }
        if b[m] == closing {
            return Some(m + 1);
        }
        m += 1;
    }
    None
}

/// `<スキーム:...>`。`rest` は `<` から。
fn autolink(rest: &str, end: bool, policy: &mut Policy<'_>) -> Parsed {
    let b = rest.as_bytes();
    let mut j = 1;
    while j < b.len() && (b[j].is_ascii_alphanumeric() || matches!(b[j], b'+' | b'.' | b'-')) {
        j += 1;
    }
    if j >= b.len() {
        return need(end);
    }
    let scheme = j - 1;
    if b[j] != b':' || !(2..=32).contains(&scheme) || !b[1].is_ascii_alphabetic() {
        return Parsed::No;
    }
    while j < b.len() {
        match b[j] {
            b'>' => {
                let out = if policy(&rest[1..j], false) {
                    rest[..=j].to_owned()
                } else {
                    String::new()
                };
                return Parsed::Done(j + 1, out);
            }
            c if c.is_ascii_whitespace() || c.is_ascii_control() || c == b'<' => {
                return Parsed::No;
            }
            _ => j += 1,
        }
    }
    need(end)
}

/// `http://`・`https://`・`www.` で始まる、そのままのURL。`rest` はその頭から。
fn bare(rest: &str, end: bool, policy: &mut Policy<'_>) -> Parsed {
    let b = rest.as_bytes();
    let mut partial = false;
    let mut matched = None;
    for (prefix, www) in [("https://", false), ("http://", false), ("www.", true)] {
        let n = prefix.len().min(b.len());
        if b[..n].eq_ignore_ascii_case(&prefix.as_bytes()[..n]) {
            if n == prefix.len() {
                matched = Some(www);
                break;
            }
            partial = true;
        }
    }
    let Some(www) = matched else {
        return if partial { need(end) } else { Parsed::No };
    };
    let mut m = 0;
    while m < b.len() && !b[m].is_ascii_whitespace() && b[m] != b'<' {
        m += 1;
    }
    if m >= b.len() && !end {
        return Parsed::More;
    }
    // 終わりの句読点と、釣り合わない閉じかっこは、URLに含めない（GFM の自動リンクと同じ）
    let mut e = m;
    loop {
        let run = &rest[..e];
        match run.as_bytes().last() {
            Some(b'?' | b'!' | b'.' | b',' | b':' | b'*' | b'_' | b'~' | b'\'' | b'"' | b';') => {
                e -= 1
            }
            Some(b')') if run.matches('(').count() < run.matches(')').count() => e -= 1,
            _ => break,
        }
    }
    let url = &rest[..e];
    let out = if policy(url, www) {
        url.to_owned()
    } else {
        String::new()
    };
    Parsed::Done(e, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SITE: &str = "https://site.example.test";

    fn allowed() -> Allowed {
        let g = Ground::new(
            "本文 [資料](https://docs.example.org/a) と https://docs.example.org/bare を見る",
            Some("https://site.example.test/ja/blog/x"),
        );
        Allowed::new([&g])
    }

    /// まとめて読んだ結果と、1文字ずつ読んだ結果（同じになる）。
    fn filtered(text: &str) -> String {
        let mut whole = Filter::new(allowed());
        let mut a = whole.push(text);
        a.push_str(&whole.finish());
        let mut each = Filter::new(allowed());
        let mut b = String::new();
        for c in text.chars() {
            b.push_str(&each.push(&c.to_string()));
        }
        b.push_str(&each.finish());
        assert_eq!(a, b, "split reading differs for {text:?}");
        a
    }

    #[test]
    fn urls_in_fragments() {
        assert_eq!(
            urls_in(
                "[a](https://x.test/1 \"t\") ![i](<https://x.test/2>) <https://x.test/3> https://x.test/4. www.x.test/5"
            ),
            vec![
                "https://x.test/1",
                "https://x.test/2",
                "https://x.test/3",
                "https://x.test/4",
                "www.x.test/5"
            ]
        );
    }

    #[test]
    fn links_and_images() {
        // 断片に書かれたURL・出典と同じサイトのURLは残る
        assert_eq!(
            filtered("[資料](https://docs.example.org/a)"),
            "[資料](https://docs.example.org/a)"
        );
        assert_eq!(
            filtered(&format!("[別](<{SITE}/b>)")),
            format!("[別](<{SITE}/b>)")
        );
        assert_eq!(
            filtered(&format!("![図]({SITE}/f.png \"題\")")),
            format!("![図]({SITE}/f.png)")
        );
        // 許さないリンクは文だけ、画像は丸ごと消える
        assert_eq!(filtered("[罠](https://evil.example.net/p) です"), "罠 です");
        assert_eq!(
            filtered("前![追跡](https://evil.example.net/x.png)後"),
            "前後"
        );
        assert_eq!(filtered("[押す](javascript:alert(1))"), "押す");
        assert_eq!(
            filtered("[相対](/path) [二重](//evil.example.net)"),
            "相対 二重"
        );
        // 同じサイトでも、表示する側で解釈が分かれうるURLは許さない
        assert_eq!(filtered(&format!("[x]({SITE}\\@evil.example.net)")), "x");
        assert_eq!(filtered(&format!("[x]({SITE}&#64;evil.example.net)")), "x");
        // 文の中のURLや画像も絞り込む
        assert_eq!(
            filtered(
                "[![i](https://evil.example.net/i.png) https://evil.example.net](https://docs.example.org/a)"
            ),
            "[ ](https://docs.example.org/a)"
        );
        // 題の中のURLは出さない
        assert_eq!(
            filtered(&format!("[x]({SITE}/a \"https://evil.example.net\")")),
            format!("[x]({SITE}/a)")
        );
    }

    #[test]
    fn citations_stay() {
        assert_eq!(
            filtered("答え [1]。次 [2] [1][2]"),
            "答え [1]。次 [2] [1][2]"
        );
        // 行の頭の `[1]: 本文` だけが、参照の定義（表示されない）
        assert_eq!(filtered("[1]: 本文"), "");
        assert_eq!(
            filtered("[1]: 本文 の説明 [2]: y"),
            "[1]: 本文 の説明 [2]: y"
        );
        assert_eq!(filtered("文 [1]: 本文"), "文 [1]: 本文");
    }

    #[test]
    fn definitions_autolinks_and_bare_urls() {
        assert_eq!(
            filtered("[r]: https://evil.example.net\n[r]: https://docs.example.org/a"),
            "\n[r]: https://docs.example.org/a"
        );
        assert_eq!(filtered("> [r]: //evil.example.net"), "> ");
        assert_eq!(
            filtered("自動 <https://evil.example.net/a> と <https://docs.example.org/a>"),
            "自動  と <https://docs.example.org/a>"
        );
        assert_eq!(
            filtered("<javascript:alert(1)> <b>太字</b>"),
            " <b>太字</b>"
        );
        assert_eq!(
            filtered(
                "見て https://evil.example.net/bare。と HTTPS://EVIL.example.net, https://docs.example.org/bare."
            ),
            "見て  , https://docs.example.org/bare."
        );
        assert_eq!(
            filtered("(https://evil.example.net/x) www.evil.example.net/y"),
            "() "
        );
        assert_eq!(filtered(&format!("({SITE}/p)")), format!("({SITE}/p)"));
        // 生の HTML の属性の中の http(s) のURLも消える
        // （GFM と同じく、URLは空白か `<` まで続く）
        assert_eq!(
            filtered("<a href=\"https://evil.example.net\">x</a>"),
            "<a href=\"</a>"
        );
        // 単語の途中の www や、http で始まる普通の語はそのまま
        assert_eq!(
            filtered("awww.x hello http header"),
            "awww.x hello http header"
        );
    }

    /// 出した文字列に、エスケープされていない `](` がない（表示する側でリンク・画像にならない）。
    fn no_link_opener(out: &str) -> bool {
        !out.replace("\\]", "").contains("](")
    }

    #[test]
    fn differences_from_the_renderer_do_not_make_links() {
        // 待てる長さを超えたリンク・画像の後に、許さない宛先が届く
        let long = "あ".repeat(HOLD_CHARS + 5);
        for text in [
            format!("[{long}](//evil.example.net)"),
            format!("![{long}](//evil.example.net/i.png)"),
            format!("[{long}](javascript:alert(1))"),
        ] {
            let out = filtered(&text);
            assert!(no_link_opener(&out), "{out}");
        }
        // コードスパンの中の `]` を閉じかっこと読んでも、表示する側ではリンクにならない
        let out = filtered("[a `]` b](//evil.example.net) と ![a `]` b](//evil.example.net/i.png)");
        assert!(no_link_opener(&out), "{out}");
        // 長すぎる自動リンクは `<` をエスケープする
        let out = filtered(&format!("<javascript:{}>", "a".repeat(HOLD_CHARS + 5)));
        assert!(out.starts_with("\\<javascript:"), "{out}");
        // `\r` も行の終わり（その後の参照の定義を絞り込む）
        assert_eq!(
            filtered("x\r[r]: //evil.example.net\r\n[r]: //evil.example.net"),
            "x\r\r\n"
        );
        // 引用はそのまま
        assert_eq!(filtered("答え [1]。[2]"), "答え [1]。[2]");
    }

    #[test]
    fn long_constructs_are_not_held_forever() {
        let mut f = Filter::new(allowed());
        let mut out = f.push("前 [");
        out.push_str(&f.push(&"あ".repeat(HOLD_CHARS + 5)));
        // 形が決まらないまま長くなったら、`[` をエスケープして文字として出す
        assert!(out.starts_with("前 \\[あ"), "{out}");
        let mut f = Filter::new(allowed());
        let mut out = f.push("前 https://evil.example.net/");
        out.push_str(&f.push(&"a".repeat(HOLD_CHARS + 5)));
        out.push_str(&f.push("aaa 後"));
        out.push_str(&f.finish());
        assert_eq!(out, "前  後");
    }
}
