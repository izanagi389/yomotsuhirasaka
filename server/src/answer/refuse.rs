//! 答えない質問（09-qa-chatbot.md「拒否する語」、INV-QA-16）。
//!
//! 当てはまった質問は回答せず、検索・埋め込み・LLM・回答キャッシュのどれにも質問文を送らない。質問ログには件数だけを残す。
//! - 拒否する語: 見えない書式文字を除き、半角カナを全角にしたうえで、回答キャッシュと同じ正規化（`cache::normalize`。全角を半角に、大文字を小文字に、空白をまとめる）を
//!   質問と語の両方にしてから、部分一致で照合する。
//! - 組み込みの種類（ワークスペースごとの ON/OFF）:
//!   - クレジットカード番号らしい並び: 13〜19桁で Luhn の検査を通るもの（空白・ハイフンの区切りを許す）
//!   - マイナンバーらしい並び: 12桁でチェックデジットが合うもの

use super::cache::normalize;

/// 拒否の設定（`answer_settings` の列）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rules {
    pub words: Vec<String>,
    pub credit_card: bool,
    pub my_number: bool,
}

/// 拒否する語の照合のための正規化。回答キャッシュの正規化に加えて、見えない書式文字を除き、半角カナを全角にする。
/// キャッシュのキーには使わない（意味の違う質問が同じ回答に当たらないよう、そちらは狭くとどめる）。
fn for_matching(text: &str) -> String {
    const KANA: &str = "ヲァィゥェォャュョッーアイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワン";
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            // 見えない書式文字（ゼロ幅の文字、双方向の制御、ソフトハイフンなど）
            '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}' => {}
            '\u{FF66}'..='\u{FF9D}' => {
                let index = c as usize - 0xFF66;
                out.extend(KANA.chars().nth(index));
            }
            // 濁点と半濁点は、前のカナと合わせる
            '\u{FF9E}' | '\u{FF9F}' | '\u{3099}' | '\u{309A}' => {
                let handakuten = matches!(c, '\u{FF9F}' | '\u{309A}');
                let joined = out.chars().last().and_then(|p| match (p, handakuten) {
                    ('ウ', false) => Some('ヴ'),
                    ('ハ' | 'ヒ' | 'フ' | 'ヘ' | 'ホ', true) => char::from_u32(p as u32 + 2),
                    (
                        'カ' | 'キ' | 'ク' | 'ケ' | 'コ' | 'サ' | 'シ' | 'ス' | 'セ' | 'ソ' | 'タ'
                        | 'チ' | 'ツ' | 'テ' | 'ト' | 'ハ' | 'ヒ' | 'フ' | 'ヘ' | 'ホ',
                        false,
                    ) => char::from_u32(p as u32 + 1),
                    _ => None,
                });
                if let Some(j) = joined {
                    out.pop();
                    out.push(j);
                }
            }
            c => out.push(c),
        }
    }
    normalize(&out)
}

impl Rules {
    /// 質問を拒否するか。
    pub fn refuses(&self, question: &str) -> bool {
        let q = for_matching(question);
        if self
            .words
            .iter()
            .map(|w| for_matching(w))
            .any(|w| !w.is_empty() && q.contains(&w))
        {
            return true;
        }
        let runs = digit_runs(question);
        (self.credit_card && runs.iter().any(|d| (13..=19).contains(&d.len()) && luhn(d)))
            || (self.my_number && runs.iter().any(|d| d.len() == 12 && my_number(d)))
    }
}

/// 区切り（空白・ハイフン）を含む数字の並び（全角の数字も）。並びごとの数字（0〜9）の列。
fn digit_runs(text: &str) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let mut pending_sep = false;
    for c in text.chars() {
        let digit = match c {
            '0'..='9' => Some(c as u8 - b'0'),
            '０'..='９' => Some((c as u32 - '０' as u32) as u8),
            _ => None,
        };
        match digit {
            Some(d) => {
                cur.push(d);
                pending_sep = false;
            }
            None if !cur.is_empty()
                && !pending_sep
                && matches!(c, ' ' | '-' | '　' | '－' | '‐') =>
            {
                pending_sep = true;
            }
            None => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                pending_sep = false;
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Luhn の検査。
fn luhn(digits: &[u8]) -> bool {
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            let d = u32::from(d);
            if i % 2 == 1 {
                let x = d * 2;
                if x > 9 { x - 9 } else { x }
            } else {
                d
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

/// マイナンバー（個人番号）のチェックデジット。上位11桁から計算した値が12桁目と合うか。
fn my_number(digits: &[u8]) -> bool {
    let (body, check) = digits.split_at(11);
    // n は下（11桁目）から数えた位置（1〜11）。重みは n ≤ 6 なら n + 1、n ≥ 7 なら n − 5
    let sum: u32 = body
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &p)| {
            let n = i as u32 + 1;
            let q = if n <= 6 { n + 1 } else { n - 5 };
            u32::from(p) * q
        })
        .sum();
    let r = sum % 11;
    let expected = if r <= 1 { 0 } else { 11 - r };
    u32::from(check[0]) == expected
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(words: &[&str], credit_card: bool, my_number: bool) -> Rules {
        Rules {
            words: words.iter().map(|w| (*w).to_owned()).collect(),
            credit_card,
            my_number,
        }
    }

    #[test]
    fn words_are_matched_after_normalizing() {
        let r = rules(&["ProjectX", "社外秘"], false, false);
        assert!(r.refuses("projectx について"));
        assert!(r.refuses("ＰＲＯＪＥＣＴＸ の件"));
        assert!(r.refuses("これは社外秘ですか"));
        assert!(!r.refuses("project x について"));
        assert!(!r.refuses("普通の質問"));
        // 空の語は何にも当たらない
        assert!(!rules(&["", "  "], false, false).refuses("何か"));
    }

    #[test]
    fn credit_cards_need_luhn() {
        let on = rules(&[], true, false);
        assert!(on.refuses("カード 4111 1111 1111 1111 です"));
        assert!(on.refuses("4111-1111-1111-1111"));
        assert!(on.refuses("４１１１１１１１１１１１１１１１"));
        // Luhn を通らない・桁が足りない・OFF
        assert!(!on.refuses("4111 1111 1111 1112"));
        assert!(!on.refuses("電話 090-1234-5678"));
        assert!(!rules(&[], false, false).refuses("4111 1111 1111 1111"));
    }

    #[test]
    fn my_numbers_need_the_check_digit() {
        // 123456789018: 上位11桁から計算したチェックデジットは 8
        let on = rules(&[], false, true);
        assert!(on.refuses("番号は 1234 5678 9018 です"));
        assert!(!on.refuses("番号は 1234 5678 9010 です"));
        assert!(!rules(&[], false, false).refuses("123456789018"));
    }

    #[test]
    fn invisible_characters_and_halfwidth_kana_do_not_slip_through() {
        let rules = Rules {
            words: vec!["社外秘".into(), "カード".into(), "ﾃﾞｰﾀ".into()],
            credit_card: false,
            my_number: false,
        };
        for q in [
            "社\u{200B}外\u{FEFF}秘の件",
            "ｶｰﾄﾞの番号",
            "データの話",
            "ﾃﾞｰﾀの話",
            "カ\u{00AD}ード",
        ] {
            assert!(rules.refuses(q), "{q}");
        }
        assert!(!rules.refuses("公開の資料"));
        assert_eq!(for_matching("ﾊﾟﾊﾞﾊ ｳﾞ"), "パバハ ヴ");
    }
}
