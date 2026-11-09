//! 答えない質問（09-qa-chatbot.md「拒否する語」、INV-QA-16）。
//!
//! 当てはまった質問は回答せず、検索・埋め込み・LLM・回答キャッシュのどれにも質問文を送らない。質問ログには件数だけを残す。
//! - 拒否する語: 回答キャッシュと同じ正規化（`cache::normalize`。全角を半角に、大文字を小文字に、空白をまとめる）を
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

impl Rules {
    /// 質問を拒否するか。
    pub fn refuses(&self, question: &str) -> bool {
        let q = normalize(question);
        if self
            .words
            .iter()
            .map(|w| normalize(w))
            .any(|w| !w.is_empty() && q.contains(&w))
        {
            return true;
        }
        let runs = digit_runs(question);
        (self.credit_card && runs.iter().any(|d| (13..=19).contains(&d.len()) && luhn(d)))
            || (self.my_number && runs.iter().any(|d| d.len() == 12 && my_number(d)))
    }
}

/// 区切り（マスクと同じ文字。`mask::is_number_separator`）を含む数字の並び（全角の数字も）。並びごとの数字（0〜9）の列。
fn digit_runs(text: &str) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    for c in text.chars() {
        let digit = match c {
            '0'..='9' => Some(c as u8 - b'0'),
            '０'..='９' => Some((c as u32 - '０' as u32) as u8),
            _ => None,
        };
        match digit {
            Some(d) => {
                cur.push(d);
            }
            None if crate::mask::is_number_separator(c) => {}
            None => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
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
    fn card_numbers_with_any_separator_are_found() {
        for q in [
            "4111−1111−1111−1111",
            "4111–1111–1111–1111",
            "4111/1111/1111/1111",
            "4111 - 1111 - 1111 - 1111",
        ] {
            assert_eq!(digit_runs(q), digit_runs("4111111111111111"), "{q}");
        }
    }

    #[test]
    fn my_numbers_need_the_check_digit() {
        // 123456789018: 上位11桁から計算したチェックデジットは 8
        let on = rules(&[], false, true);
        assert!(on.refuses("番号は 1234 5678 9018 です"));
        assert!(!on.refuses("番号は 1234 5678 9010 です"));
        assert!(!rules(&[], false, false).refuses("123456789018"));
    }
}
