//! 支援の出力の言語（小さなLLMが、指示と違う言語で書くことがあるため）。
//!
//! - プロンプトでは「下書きの言語で」ではなく、ロケールから決めた言語名を名指しする（[`name`]）。
//!   チャットボットの回答の指示（`answer::prompt`）も、同じ名前を使う。
//! - 指示は保証ではないので、CMS のコードで確かめる（原則4。[`written_in`]）。確かめるのは規則で判定できる言語だけで、
//!   いまは日本語だけ。ほかのロケールは確かめずに通す。
//! - 確かめられなかった出力は、呼び出し元が1回だけ指示を強めて取り直し（[`reminder`]）、それでも違えば表示しない。

/// ロケール（BCP 47 の先頭の言語の部分）から、プロンプトに書く言語名。知らない言語はロケールのまま名指しする。
pub fn name(locale: &str) -> String {
    let lang = primary(locale);
    let known = match lang.as_str() {
        "ja" => "Japanese (日本語)",
        "en" => "English",
        "zh" => "Chinese (中文)",
        "ko" => "Korean (한국어)",
        "fr" => "French (français)",
        "de" => "German (Deutsch)",
        "es" => "Spanish (español)",
        "pt" => "Portuguese (português)",
        "it" => "Italian (italiano)",
        _ => "",
    };
    if known.is_empty() {
        format!("the language of the locale `{locale}`")
    } else {
        known.to_owned()
    }
}

/// 取り直すときにユーザーのメッセージの最後に足す念押し。
pub fn reminder(locale: &str) -> String {
    format!(
        "\n\nReminder: write every explanation in {}. Do not write in any other language.",
        name(locale)
    )
}

/// 出力がロケールの言語で書かれているとみなせるか。規則で判定できない言語は true。
pub fn written_in(locale: &str, text: &str) -> bool {
    match primary(locale).as_str() {
        "ja" => looks_japanese(text),
        _ => true,
    }
}

fn primary(locale: &str) -> String {
    locale
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// 中国語の簡体字にしかなく、日本語では使わない字（中国語の文が混じったことの目印）。
const SIMPLIFIED_ONLY: &[char] = &[
    '这', '们', '个', '说', '么', '还', '进', '对', '从', '让', '时', '为', '关', '问', '题', '应',
    '该', '实', '际', '现', '经', '过', '样', '吗', '呢', '视', '频', '库', '费', '链', '计', '价',
];

/// 日本語の文とみなせるか。
/// - 引用（「」『』“”""``）の中は、記事の題名や本文の写しなので数えない
/// - 引用の外に、かなが1字以上あり、漢字がかなの5倍を超えない（中国語の文は、かながほぼない）
/// - 引用の外の英字が、かなと漢字の合計の2倍を超えない（英語の文が混じっていない）
/// - 簡体字にしかない字がない
fn looks_japanese(text: &str) -> bool {
    let outside = outside_quotes(text);
    let (mut kana, mut han, mut latin) = (0usize, 0usize, 0usize);
    for c in outside.chars() {
        if SIMPLIFIED_ONLY.contains(&c) {
            return false;
        }
        match c {
            '\u{3041}'..='\u{309f}' | '\u{30a0}'..='\u{30ff}' | '\u{31f0}'..='\u{31ff}' => {
                kana += 1
            }
            '\u{4e00}'..='\u{9fff}' | '\u{3400}'..='\u{4dbf}' => han += 1,
            'a'..='z' | 'A'..='Z' => latin += 1,
            _ => {}
        }
    }
    kana > 0 && han <= kana * 5 && latin <= (kana + han) * 2
}

/// 引用の中を除いた文字列。閉じていない引用は、そこから後ろを引用とみなす。
fn outside_quotes(text: &str) -> String {
    let mut out = String::new();
    let mut closing: Vec<char> = Vec::new();
    for c in text.chars() {
        if let Some(&end) = closing.last() {
            if c == end {
                closing.pop();
            }
            continue;
        }
        match c {
            '「' => closing.push('」'),
            '『' => closing.push('』'),
            '“' => closing.push('”'),
            '"' => closing.push('"'),
            '`' => closing.push('`'),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_language_of_the_locale() {
        assert_eq!(name("ja"), "Japanese (日本語)");
        assert_eq!(name("ja-JP"), "Japanese (日本語)");
        assert_eq!(name("EN_us"), "English");
        assert_eq!(name("sw"), "the language of the locale `sw`");
        assert!(reminder("ja").contains("Japanese (日本語)"));
    }

    #[test]
    fn japanese_explanations_pass() {
        for ok in [
            "下書きでは料金が月500円ですが、断片では月300円です。",
            "leafletは無料の地図ライブラリであるため、[1]の説明と矛盾します。",
            "Nuxt.js の vue2-leaflet を使う手順が違います。",
            "価格は変わりやすい",
            "バージョン",
            "下書きの手順は断片の手順と衝突しています。給与の計算に関与する値が違います。",
        ] {
            assert!(written_in("ja", ok), "{ok}");
        }
    }

    #[test]
    fn chinese_and_english_explanations_fail() {
        for bad in [
            // 実際に出た例（引用の中のかなは数えない）
            "使用方法中有关Mac上的「ショートカット」的信息与[6] Nuxt.jsで「leaflet」を使ってみる！中的模板不一致。",
            "这个说明与草稿不一致。",
            "The draft says the price is 500 yen but the fragment says 300 yen.",
            "「leaflet」は有料です。This contradicts the statement that leaflet is a free map library.",
            "价格不一致",
            "",
        ] {
            assert!(!written_in("ja", bad), "{bad}");
        }
    }

    #[test]
    fn other_locales_are_not_checked() {
        assert!(written_in("en", "これは日本語です"));
        assert!(written_in("fr", "价格不一致"));
    }
}
