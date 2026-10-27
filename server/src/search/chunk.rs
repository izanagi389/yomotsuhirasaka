//! チャンク分割（08-search-and-related.md「チャンク分割（Q28）」）。
//!
//! - 本文（richtext）は、変換器が作った Markdown（AI向け素材と同じ。Q36, Q56）を見出しで区切る。
//!   見出しは `## 見出し {#アンカーID}` の形で出る。パンくずは「タイトル ＞ 大見出し ＞ 小見出し」。
//! - text のフィールドは、それぞれを1つの短いセクションとして扱う。
//! - 長すぎるセクション（[`MAX_CHARS`] 超）は段落（ブロック）の境目で、1つの段落が長すぎれば文の境目で、それでも長ければ文字数で分ける。
//! - 短すぎるセクション（[`MIN_CHARS`] 未満）は次とつなぐ（最後なら前とつなぐ）。つないだチャンクのアンカーとパンくずは、先のもの。
//! - 文字数は Unicode のコードポイントで数える。値を変えるとチャンクと埋め込みの作り直しになるので、設定にしない。

/// 1つのチャンクの文字数の目安の上限。
pub const MAX_CHARS: usize = 1200;
/// これより短いセクションは、次とつなぐ。
pub const MIN_CHARS: usize = 200;

/// チャンクの元になるフィールドの値。
pub enum Source<'a> {
    /// text のフィールド（そのままの文字列）。
    Text(&'a str),
    /// richtext のフィールドを変換器で Markdown にしたもの。
    Markdown(&'a str),
}

/// チャンク。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// セクションのアンカーID（見出しがなければ None）。
    pub anchor: Option<String>,
    /// 「タイトル ＞ 大見出し ＞ 小見出し」（タイトルは、検索の対象のときだけ）。
    pub breadcrumb: Vec<String>,
    /// 本文（Markdown。見出しの行は、見出しの文字だけにする）。
    pub text: String,
}

struct Section {
    anchor: Option<String>,
    breadcrumb: Vec<String>,
    blocks: Vec<String>,
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

/// `## 見出し {#id}` を（レベル、見出しの文字、アンカーID）にする。見出しでなければ None。
fn heading(block: &str) -> Option<(usize, String, Option<String>)> {
    if block.contains('\n') {
        return None;
    }
    let level = block.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&level) || !block[level..].starts_with(' ') {
        return None;
    }
    let rest = block[level + 1..].trim();
    if let Some(open) = rest.rfind(" {#")
        && rest.ends_with('}')
    {
        let id = &rest[open + 3..rest.len() - 1];
        if !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Some((level, rest[..open].trim().to_owned(), Some(id.to_owned())));
        }
    }
    Some((level, rest.to_owned(), None))
}

/// Markdown を空行で区切ったブロックにする（コードのフェンスの中の空行では区切らない）。
fn blocks(markdown: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for line in markdown.lines() {
        let t = line.trim_start();
        let run = |c: char| t.chars().take_while(|x| *x == c).count();
        match fence {
            None => {
                for c in ['`', '~'] {
                    if run(c) >= 3 {
                        fence = Some((c, run(c)));
                    }
                }
            }
            Some((c, n)) if run(c) >= n && t.trim_end().chars().all(|x| x == c) => fence = None,
            Some(_) => {}
        }
        if fence.is_none() && line.trim().is_empty() {
            if !current.is_empty() {
                out.push(current.join("\n"));
                current.clear();
            }
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        out.push(current.join("\n"));
    }
    out
}

/// 長すぎる段落を、文の境目で分ける（それでも長ければ文字数で分ける）。
fn split_long(block: &str) -> Vec<String> {
    let mut sentences: Vec<String> = Vec::new();
    let mut current = String::new();
    let all: Vec<char> = block.chars().collect();
    for (i, c) in all.iter().enumerate() {
        current.push(*c);
        let end_cjk = matches!(c, '。' | '．' | '！' | '？');
        let end_latin =
            matches!(c, '.' | '!' | '?') && all.get(i + 1).is_none_or(|n| n.is_whitespace());
        if end_cjk || end_latin {
            sentences.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        sentences.push(current);
    }
    let mut out: Vec<String> = Vec::new();
    let mut acc = String::new();
    for s in sentences {
        if !acc.is_empty() && chars(&acc) + chars(&s) > MAX_CHARS {
            out.push(std::mem::take(&mut acc));
        }
        acc.push_str(&s);
        // 1つの文が長すぎれば、文字数で切る
        while chars(&acc) > MAX_CHARS {
            let head: String = acc.chars().take(MAX_CHARS).collect();
            let tail: String = acc.chars().skip(MAX_CHARS).collect();
            out.push(head);
            acc = tail;
        }
    }
    if !acc.trim().is_empty() {
        out.push(acc);
    }
    out.into_iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

/// フィールドの値から、セクションを作る。
fn sections(title: Option<&str>, sources: &[Source<'_>]) -> Vec<Section> {
    let root: Vec<String> = title.map(|t| vec![t.to_owned()]).unwrap_or_default();
    let mut out = Vec::new();
    for source in sources {
        match source {
            Source::Text(text) => {
                if !text.trim().is_empty() {
                    out.push(Section {
                        anchor: None,
                        breadcrumb: root.clone(),
                        blocks: vec![text.trim().to_owned()],
                    });
                }
            }
            Source::Markdown(markdown) => {
                // 見出しの入れ子（レベル、見出しの文字）
                let mut stack: Vec<(usize, String)> = Vec::new();
                let mut current = Section {
                    anchor: None,
                    breadcrumb: root.clone(),
                    blocks: Vec::new(),
                };
                for block in blocks(markdown) {
                    if let Some((level, text, anchor)) = heading(&block) {
                        if !current.blocks.is_empty() {
                            out.push(current);
                        }
                        while stack.last().is_some_and(|(l, _)| *l >= level) {
                            stack.pop();
                        }
                        stack.push((level, text.clone()));
                        let mut breadcrumb = root.clone();
                        breadcrumb.extend(stack.iter().map(|(_, t)| t.clone()));
                        current = Section {
                            anchor,
                            breadcrumb,
                            blocks: vec![text],
                        };
                    } else {
                        current.blocks.push(block);
                    }
                }
                if !current.blocks.is_empty() {
                    out.push(current);
                }
            }
        }
    }
    out
}

/// フィールドの値をチャンクにする。`title` は、タイトルのフィールドが検索の対象のときだけ渡す（INV-SRC-03）。
pub fn chunks(title: Option<&str>, sources: &[Source<'_>]) -> Vec<Chunk> {
    // 長すぎるセクションを分ける
    let mut pieces: Vec<Chunk> = Vec::new();
    for section in sections(title, sources) {
        let mut acc: Vec<String> = Vec::new();
        let flush = |acc: &mut Vec<String>, pieces: &mut Vec<Chunk>| {
            if !acc.is_empty() {
                pieces.push(Chunk {
                    anchor: section.anchor.clone(),
                    breadcrumb: section.breadcrumb.clone(),
                    text: acc.join("\n\n"),
                });
                acc.clear();
            }
        };
        for block in &section.blocks {
            let parts = if chars(block) > MAX_CHARS {
                split_long(block)
            } else {
                vec![block.clone()]
            };
            for part in parts {
                let size: usize = acc.iter().map(|b| chars(b) + 2).sum();
                if !acc.is_empty() && size + chars(&part) > MAX_CHARS {
                    flush(&mut acc, &mut pieces);
                }
                acc.push(part);
            }
        }
        flush(&mut acc, &mut pieces);
    }
    // 短すぎるものを次とつなぐ（最後なら前とつなぐ）
    let mut out: Vec<Chunk> = Vec::new();
    let mut pending: Option<Chunk> = None;
    for piece in pieces {
        let piece = match pending.take() {
            Some(short) => Chunk {
                text: format!("{}\n\n{}", short.text, piece.text),
                ..short
            },
            None => piece,
        };
        if chars(&piece.text) < MIN_CHARS {
            pending = Some(piece);
        } else {
            out.push(piece);
        }
    }
    if let Some(short) = pending {
        match out.last_mut() {
            Some(last) => last.text = format!("{}\n\n{}", last.text, short.text),
            None => out.push(short),
        }
    }
    out
}

/// 埋め込みに渡す文字列（パンくずと本文）。
pub fn embedding_input(chunk: &Chunk) -> String {
    if chunk.breadcrumb.is_empty() {
        chunk.text.clone()
    } else {
        format!("{}\n{}", chunk.breadcrumb.join(" > "), chunk.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn long(n: usize, unit: &str) -> String {
        unit.repeat(n)
    }

    #[test]
    fn splits_by_headings_with_anchors_and_breadcrumbs() {
        let intro = long(30, "導入の文です。");
        let a = long(30, "第一の節の文です。");
        let b = long(30, "小さな節の文です。");
        let md =
            format!("{intro}\n\n## 第一の節 {{#first}}\n\n{a}\n\n### 小さな節 {{#small}}\n\n{b}");
        let c = chunks(Some("記事"), &[Source::Markdown(&md)]);
        assert_eq!(c.len(), 3, "{c:#?}");
        assert_eq!(
            (c[0].anchor.as_deref(), c[0].breadcrumb.clone()),
            (None, vec!["記事".to_owned()])
        );
        assert_eq!(c[1].anchor.as_deref(), Some("first"));
        assert_eq!(c[1].breadcrumb, vec!["記事", "第一の節"]);
        assert!(
            c[1].text.starts_with("第一の節\n\n"),
            "見出しの行は見出しの文字だけ: {}",
            c[1].text
        );
        assert!(!c[1].text.contains("{#"));
        assert_eq!(c[2].anchor.as_deref(), Some("small"));
        assert_eq!(c[2].breadcrumb, vec!["記事", "第一の節", "小さな節"]);
    }

    #[test]
    fn title_is_left_out_when_not_given() {
        let md = format!("## 節 {{#s}}\n\n{}", long(30, "本文の文です。"));
        let c = chunks(None, &[Source::Markdown(&md)]);
        assert_eq!(c[0].breadcrumb, vec!["節"]);
        assert_eq!(embedding_input(&c[0]), format!("節\n{}", c[0].text));
    }

    #[test]
    fn long_sections_split_at_paragraphs_then_sentences() {
        let para = long(50, "あいうえおかきくけこ。"); // 550文字の段落
        let md = format!("## 節 {{#s}}\n\n{para}\n\n{para}\n\n{para}");
        let c = chunks(None, &[Source::Markdown(&md)]);
        assert!(c.len() >= 2, "{c:#?}");
        assert!(
            c.iter().all(|x| chars(&x.text) <= MAX_CHARS + MIN_CHARS),
            "{c:#?}"
        );
        assert!(c.iter().all(|x| x.anchor.as_deref() == Some("s")));
        // 1つの段落が長すぎれば、文の境目で分ける
        let huge = long(300, "これは長い文です。"); // 2700文字
        let c = chunks(None, &[Source::Markdown(&huge)]);
        assert!(c.len() >= 3, "{}", c.len());
        assert!(
            c.iter().all(|x| x.text.ends_with('。')),
            "文の途中で切れている"
        );
        // 文の区切りがなければ、文字数で切る
        let c = chunks(None, &[Source::Markdown(&"あ".repeat(3000))]);
        assert!(c.iter().all(|x| chars(&x.text) <= MAX_CHARS + MIN_CHARS));
        assert_eq!(c.iter().map(|x| chars(&x.text)).sum::<usize>(), 3000);
    }

    #[test]
    fn short_sections_are_merged_with_the_next() {
        let md = format!(
            "## 短い {{#a}}\n\n少し。\n\n## 長い {{#b}}\n\n{}",
            long(30, "長い節の文です。")
        );
        let c = chunks(
            None,
            &[Source::Text("副題のような短い値"), Source::Markdown(&md)],
        );
        assert_eq!(c.len(), 1, "{c:#?}");
        assert_eq!(c[0].anchor, None, "つないだチャンクのアンカーは先のもの");
        assert!(
            c[0].text
                .starts_with("副題のような短い値\n\n短い\n\n少し。")
        );
        // 最後が短ければ前とつなぐ
        let md = format!(
            "{}\n\n## 末尾 {{#z}}\n\n短い。",
            long(30, "前の節の文です。")
        );
        let c = chunks(None, &[Source::Markdown(&md)]);
        assert_eq!(c.len(), 1);
        assert!(c[0].text.ends_with("末尾\n\n短い。"));
    }

    #[test]
    fn blank_lines_in_code_fences_do_not_split_blocks() {
        let b = blocks("```\na\n\nb\n```\n\nafter");
        assert_eq!(b, vec!["```\na\n\nb\n```".to_owned(), "after".to_owned()]);
        assert_eq!(heading("#no space"), None);
        assert_eq!(
            heading("## タイトル {#x-1}"),
            Some((2, "タイトル".into(), Some("x-1".into())))
        );
        assert_eq!(heading("## {#} だけ"), Some((2, "{#} だけ".into(), None)));
    }

    #[test]
    fn empty_input_has_no_chunks() {
        assert!(chunks(Some("t"), &[Source::Text("  "), Source::Markdown("")]).is_empty());
    }
}
