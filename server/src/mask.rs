//! 訪問者が書いた文のマスク（13-security.md「外部のLLMに送る前のマスク」、Q169）。
//!
//! 外部のLLMへの送信と埋め込みの作成の前に、決まった形の個人情報を伏せる。規則はここ1か所に置き、サイト内検索の検索語（M3）と、
//! チャットボットの質問文・質問ログ（M4）で同じものを使う。
//! - メールアドレス → `[email]`
//! - 10桁以上の数字の並び（電話番号・カード番号・口座番号など。空白・ハイフン・括弧・`+`・`.` で区切られていてもよい）→ `[number]`
//! - 全角の数字と `＠` も同じように扱う。
//!
//! 氏名や住所のような自由な書き方は伏せられない（マスクの限界。管理画面で知らせる）。

const EMAIL: &str = "[email]";
const NUMBER: &str = "[number]";
/// これだけの桁数の数字の並びを伏せる（日本の電話番号は10〜11桁）。
const MIN_DIGITS: usize = 10;

fn is_digit(c: char) -> bool {
    c.is_ascii_digit() || ('０'..='９').contains(&c)
}

fn is_at(c: char) -> bool {
    c == '@' || c == '＠'
}

fn is_local(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-')
}

fn is_domain(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-')
}

/// 数字の並びの区切りとして許す文字。
fn is_number_separator(c: char) -> bool {
    matches!(
        c,
        ' ' | '-' | '(' | ')' | '+' | '.' | '－' | '　' | '（' | '）' | '＋' | '‐' | 'ー'
    )
}

/// メールアドレスの範囲（`@` の位置から）。
fn email_at(chars: &[char], at: usize) -> Option<(usize, usize)> {
    let mut start = at;
    while start > 0 && is_local(chars[start - 1]) {
        start -= 1;
    }
    let mut end = at + 1;
    while end < chars.len() && is_domain(chars[end]) {
        end += 1;
    }
    // 末尾の . や - は含めない
    while end > at + 1 && matches!(chars[end - 1], '.' | '-') {
        end -= 1;
    }
    let domain: String = chars[at + 1..end].iter().collect();
    let tld = domain.rsplit('.').next().unwrap_or("");
    let valid = start < at
        && domain.contains('.')
        && tld.len() >= 2
        && tld.chars().all(|c| c.is_ascii_alphabetic());
    valid.then_some((start, end))
}

/// 決まった形の個人情報を伏せる。
pub fn mask(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    // メールアドレスを先に伏せる（数字を含むアドレスを、数字の並びとして半端に伏せないように）
    let mut spans: Vec<(usize, usize, &str)> = Vec::new();
    for (at, c) in chars.iter().enumerate() {
        if is_at(*c)
            && let Some((s, e)) = email_at(&chars, at)
            && spans.last().is_none_or(|(_, last_end, _)| *last_end <= s)
        {
            spans.push((s, e, EMAIL));
        }
    }
    let mut next_span = 0;
    while i < chars.len() {
        if let Some((s, e, label)) = spans.get(next_span).copied()
            && s == i
        {
            out.push_str(label);
            i = e;
            next_span += 1;
            continue;
        }
        if is_digit(chars[i]) {
            // 区切りを含む数字の並びの終わりを探す（区切りで終わらない）
            let limit = spans.get(next_span).map_or(chars.len(), |(s, _, _)| *s);
            let mut j = i;
            let mut digits = 0;
            let mut last_digit = i;
            while j < limit && (is_digit(chars[j]) || is_number_separator(chars[j])) {
                if is_digit(chars[j]) {
                    digits += 1;
                    last_digit = j;
                }
                j += 1;
            }
            if digits >= MIN_DIGITS {
                // 直前の + や ( も一緒に伏せる
                while out.ends_with(['+', '(', '＋', '（']) {
                    out.pop();
                }
                out.push_str(NUMBER);
                i = last_digit + 1;
                continue;
            }
            // 短い数字は、そのまま
            for c in &chars[i..=last_digit] {
                out.push(*c);
            }
            i = last_digit + 1;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails_are_masked() {
        assert_eq!(
            mask("連絡は taro.yamada+cms@example.co.jp まで"),
            "連絡は [email] まで"
        );
        assert_eq!(
            mask("ｔ＠example.com"),
            "ｔ＠example.com",
            "全角の英字はアドレスにしない"
        );
        assert_eq!(mask("abc＠example.com です"), "[email] です");
        assert_eq!(
            mask("a@b.c"),
            "a@b.c",
            "トップレベルドメインが短すぎるものはアドレスにしない"
        );
        assert_eq!(mask("@user と a@example.org."), "@user と [email].");
        assert_eq!(mask("x1234567890@example.com"), "[email]");
    }

    #[test]
    fn long_digit_runs_are_masked() {
        assert_eq!(mask("電話 090-1234-5678 まで"), "電話 [number] まで");
        assert_eq!(mask("+81 (3) 1234-5678"), "[number]");
        assert_eq!(mask("０３－１２３４－５６７８に"), "[number]に");
        assert_eq!(mask("カード 4111 1111 1111 1111"), "カード [number]");
        // 短い数字はそのまま（年月日、件数など）
        assert_eq!(mask("2026-10-28 に 3 件"), "2026-10-28 に 3 件");
        assert_eq!(mask("123456789"), "123456789");
        // 区切りで終わる数字の並びは、区切りを残す
        assert_eq!(mask("番号 0312345678- です"), "番号 [number]- です");
    }

    #[test]
    fn plain_text_is_unchanged() {
        assert_eq!(mask("PostgreSQL の全文検索"), "PostgreSQL の全文検索");
        assert_eq!(mask(""), "");
    }
}
