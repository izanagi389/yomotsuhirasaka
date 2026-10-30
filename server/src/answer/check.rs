//! LLMの回答の検査（09-qa-chatbot.md「根拠と判定」「ストリーミング」、原則4）。LLMへの指示は保証ではないので、ここで守る。
//!
//! - 回答の1行目は判定の先行宣言。形に合わない、「答えられない」、実在しない出典番号を含む宣言は、本文を配信せずに
//!   「情報なし」にする（INV-QA-04）。
//! - 宣言を確かめた後で、本文を配信する。本文は [`MAX_ANSWER_CHARS`] 文字で打ち切る（INV-QA-13）。
//! - 最後に、本文の中の引用記号を照合する。宣言した番号の引用が1つもなければ「情報なし」に差し替える（INV-QA-01。無効にできない）。
//!   宣言していない番号の引用記号は、返す本文から取り除く。ストリーミングでは配信した後なので、最終判定で「規則違反を検知した」と示す。

use super::format;

/// 回答の本文の長さの上限（文字。Unicode のコードポイント）。
pub const MAX_ANSWER_CHARS: usize = 4000;
/// 宣言の行の長さの上限（文字）。改行がないまま超えたら、宣言の形に合わないものとして扱う。
const MAX_DECLARATION_CHARS: usize = 200;

/// 検査の途中で、呼び出し側がすること。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// 宣言を確かめた（答えられる）。使う出典の番号。
    Answerable(Vec<usize>),
    /// 宣言が「答えられない」か、規則に合わない。本文は配信しない。LLMの応答は読まなくてよい。
    Refused,
    /// 本文の断片を配信する。
    Delta(String),
    /// 本文が長さの上限に達した。LLMの応答は読まなくてよい。
    Full,
}

/// 最終判定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// 回答できた。`text` は宣言していない引用記号を取り除いた本文、`cited` は引用した出典の番号（小さい順）。
    Answered {
        text: String,
        cited: Vec<usize>,
        /// 宣言していない番号の引用記号があった。
        violation: bool,
    },
    NoInformation,
}

#[derive(Debug)]
enum Phase {
    Declaration,
    Body,
    Done,
}

/// LLMの回答を、届いた順に検査する。
#[derive(Debug)]
pub struct Checker {
    /// プロンプトに入れた断片の数（番号は 1..=fragments）。
    fragments: usize,
    phase: Phase,
    head: String,
    declared: Vec<usize>,
    body: String,
    body_chars: usize,
    truncated: bool,
}

impl Checker {
    pub fn new(fragments: usize) -> Self {
        Self {
            fragments,
            phase: Phase::Declaration,
            head: String::new(),
            declared: Vec::new(),
            body: String::new(),
            body_chars: 0,
            truncated: false,
        }
    }

    /// 本文を長さの上限で打ち切ったか。
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// LLMの回答の断片を1つ読む。
    pub fn push(&mut self, text: &str) -> Vec<Step> {
        let mut out = Vec::new();
        match self.phase {
            Phase::Done => {}
            Phase::Declaration => {
                self.head.push_str(text);
                if let Some(pos) = self.head.find('\n') {
                    let rest = self.head.split_off(pos + 1);
                    let line = std::mem::take(&mut self.head);
                    match self.declare(&line) {
                        Some(step) => {
                            out.push(step);
                            self.phase = Phase::Body;
                            out.extend(self.push(&rest));
                        }
                        None => {
                            out.push(Step::Refused);
                            self.phase = Phase::Done;
                        }
                    }
                } else if self.head.chars().count() > MAX_DECLARATION_CHARS {
                    out.push(Step::Refused);
                    self.phase = Phase::Done;
                }
            }
            Phase::Body => {
                let room = MAX_ANSWER_CHARS - self.body_chars;
                let taken: String = text.chars().take(room).collect();
                let n = taken.chars().count();
                self.body_chars += n;
                self.body.push_str(&taken);
                if !taken.is_empty() {
                    out.push(Step::Delta(taken));
                }
                if n < text.chars().count() {
                    self.truncated = true;
                    self.phase = Phase::Done;
                    out.push(Step::Full);
                }
            }
        }
        out
    }

    /// 宣言の行を確かめる。答えられるなら Step::Answerable、そうでなければ None。
    fn declare(&mut self, line: &str) -> Option<Step> {
        let sources = format::parse_declaration(line)?;
        if sources.is_empty() || sources.iter().any(|&n| n == 0 || n > self.fragments) {
            return None;
        }
        self.declared = sources.clone();
        Some(Step::Answerable(sources))
    }

    /// LLMの回答が終わった（または読むのをやめた）ときの最終判定。宣言だけで本文がなければ「情報なし」。
    pub fn finish(&mut self) -> (Option<Step>, Verdict) {
        let mut early = None;
        if let Phase::Declaration = self.phase {
            // 改行のないまま終わった: 1行だけの回答は宣言として確かめる（本文はない）
            let line = std::mem::take(&mut self.head);
            early = Some(match self.declare(&line) {
                Some(step) => step,
                None => Step::Refused,
            });
        }
        self.phase = Phase::Done;
        if self.declared.is_empty() {
            return (early, Verdict::NoInformation);
        }
        let mut text = String::with_capacity(self.body.len());
        let mut cited: Vec<usize> = Vec::new();
        let mut violation = false;
        let mut at = 0;
        for (range, n) in format::citations(&self.body) {
            text.push_str(&self.body[at..range.start]);
            match n.filter(|n| self.declared.contains(n)) {
                Some(n) => {
                    text.push_str(&self.body[range.clone()]);
                    if !cited.contains(&n) {
                        cited.push(n);
                    }
                }
                None => violation = true,
            }
            at = range.end;
        }
        text.push_str(&self.body[at..]);
        // 打ち切りで途中になった引用記号（`[1` など）は残さない
        if self.truncated
            && let Some(open) = text.rfind('[')
            && !text[open..].contains(']')
        {
            text.truncate(open);
        }
        if cited.is_empty() {
            return (early, Verdict::NoInformation);
        }
        cited.sort_unstable();
        (
            early,
            Verdict::Answered {
                text: text.trim().to_owned(),
                cited,
                violation,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(fragments: usize, pieces: &[&str]) -> (Vec<Step>, Verdict) {
        let mut c = Checker::new(fragments);
        let mut steps = Vec::new();
        for p in pieces {
            steps.extend(c.push(p));
        }
        let (early, verdict) = c.finish();
        steps.extend(early);
        (steps, verdict)
    }

    fn deltas(steps: &[Step]) -> String {
        steps
            .iter()
            .filter_map(|s| match s {
                Step::Delta(t) => Some(t.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn declaration_split_across_pieces_then_body() {
        let (steps, verdict) = run(2, &["ANSWERABLE: y", "es; SOURCES: 2\n答え", "です [2]。"]);
        assert_eq!(steps[0], Step::Answerable(vec![2]));
        assert_eq!(deltas(&steps), "答えです [2]。");
        assert_eq!(
            verdict,
            Verdict::Answered {
                text: "答えです [2]。".into(),
                cited: vec![2],
                violation: false
            }
        );
    }

    #[test]
    fn refusals_send_no_body() {
        let long_head = "x".repeat(300);
        for pieces in [
            &["ANSWERABLE: no; SOURCES: none\n本文 [1]"][..],
            &["ANSWERABLE: yes; SOURCES: 3\n本文 [3]"][..],
            &["ANSWERABLE: yes; SOURCES: 0\n本文 [0]"][..],
            &["Here is the answer:\nANSWERABLE: yes; SOURCES: 1\n[1]"][..],
            &[long_head.as_str()][..],
        ] {
            let (steps, verdict) = run(2, pieces);
            assert!(steps.contains(&Step::Refused), "{pieces:?}");
            assert_eq!(deltas(&steps), "", "{pieces:?}");
            assert_eq!(verdict, Verdict::NoInformation, "{pieces:?}");
        }
    }

    #[test]
    fn no_citation_is_no_information() {
        let (steps, verdict) = run(2, &["ANSWERABLE: yes; SOURCES: 1\n引用のない本文"]);
        assert_eq!(deltas(&steps), "引用のない本文");
        assert_eq!(verdict, Verdict::NoInformation);
        // 宣言していない番号だけの引用も、引用がないのと同じ
        let (_, verdict) = run(2, &["ANSWERABLE: yes; SOURCES: 1\n本文 [2]"]);
        assert_eq!(verdict, Verdict::NoInformation);
    }

    #[test]
    fn undeclared_citations_are_removed_and_flagged() {
        let (_, verdict) = run(3, &["ANSWERABLE: yes; SOURCES: 1\nA [1] B [3] C [9]"]);
        assert_eq!(
            verdict,
            Verdict::Answered {
                text: "A [1] B  C".into(),
                cited: vec![1],
                violation: true
            }
        );
    }

    #[test]
    fn body_is_cut_at_the_limit() {
        let long = "あ".repeat(MAX_ANSWER_CHARS + 10);
        let mut c = Checker::new(1);
        let mut steps = c.push("ANSWERABLE: yes; SOURCES: 1\n[1] ");
        steps.extend(c.push(&long));
        assert!(steps.contains(&Step::Full));
        assert_eq!(deltas(&steps).chars().count(), MAX_ANSWER_CHARS);
        assert!(c.truncated());
        // 打ち切った後は読まない
        assert!(c.push("more").is_empty());
        match c.finish().1 {
            Verdict::Answered { text, .. } => assert!(text.chars().count() <= MAX_ANSWER_CHARS),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn dangling_citation_after_cut_is_dropped() {
        let mut c = Checker::new(1);
        c.push("ANSWERABLE: yes; SOURCES: 1\n[1] ");
        let filler = "い".repeat(MAX_ANSWER_CHARS - 6);
        c.push(&format!("{filler}[1] [1"));
        match c.finish().1 {
            Verdict::Answered { text, .. } => assert!(text.ends_with('い'), "{text}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn declaration_only_reply() {
        let (steps, verdict) = run(1, &["ANSWERABLE: yes; SOURCES: 1"]);
        assert_eq!(steps, vec![Step::Answerable(vec![1])]);
        assert_eq!(verdict, Verdict::NoInformation);
    }
}
