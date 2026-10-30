//! 回答の語り口と、訪問者向けの定型文（09-qa-chatbot.md「語り口」、Q24, Q25、03-content-and-editing.md「訪問者向けの文言」）。
//!
//! - 構造化された設定: 丁寧さ（[`Politeness`]）と回答の長さ（[`Length`]）。プロンプトでの指示だけに使う（出力の上限と、
//!   CMSが打ち切る長さは変えない。予算の見積もりをぶらさないため）。
//! - 自由記述のシステムプロンプト: 使うかどうかのスイッチ（既定は使わない）と、文（[`MAX_FREE_PROMPT_CHARS`] 文字まで）。
//!   使わない設定でも文は残し、LLMには送らない。プロンプトでは「語り口」の枠にだけ入れ、CMSの規則をその後に置く。
//!   引用の検証は、自由記述の有無にかかわらず行う（`check`。INV-QA-03）。
//! - 「情報なし」の文: コンテンツのロケールごと（`answer_phrases`）。テンプレートのロケールだけを受け付ける。

use std::collections::BTreeMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use sqlx::PgConnection;

use crate::schema::manifest::Manifest;

/// 自由記述のシステムプロンプトの長さの上限（文字）。
pub const MAX_FREE_PROMPT_CHARS: usize = 2000;
/// 「情報なし」の文の長さの上限（文字）。
pub const MAX_PHRASE_CHARS: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Politeness {
    #[default]
    Polite,
    Casual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Length {
    Short,
    #[default]
    Standard,
    Detailed,
}

impl Politeness {
    fn as_str(self) -> &'static str {
        match self {
            Self::Polite => "polite",
            Self::Casual => "casual",
        }
    }

    fn parse(s: &str) -> Self {
        if s == "casual" {
            Self::Casual
        } else {
            Self::Polite
        }
    }
}

impl Length {
    fn as_str(self) -> &'static str {
        match self {
            Self::Short => "short",
            Self::Standard => "standard",
            Self::Detailed => "detailed",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "short" => Self::Short,
            "detailed" => Self::Detailed,
            _ => Self::Standard,
        }
    }
}

/// プロンプトに効く語り口（自由記述は、使う設定のときだけ）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tone {
    pub politeness: Politeness,
    pub length: Length,
    pub free_prompt: Option<String>,
}

impl Tone {
    /// 回答キャッシュの設定のハッシュに入れるもの（語り口を変えたら、前のキャッシュに当たらない）。
    pub fn fingerprint(&self) -> String {
        let mut h = Sha256::new()
            .chain_update(self.politeness.as_str().as_bytes())
            .chain_update([0])
            .chain_update(self.length.as_str().as_bytes())
            .chain_update([0]);
        if let Some(text) = &self.free_prompt {
            h.update([1]);
            h.update(text.as_bytes());
        }
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }
}

type Row = (String, String, bool, String);

async fn row(conn: &mut PgConnection) -> Result<Row> {
    Ok(sqlx::query_as(
        "SELECT politeness, length, free_prompt_enabled, free_prompt FROM answer_settings",
    )
    .fetch_one(&mut *conn)
    .await?)
}

/// 拒否する語の数と長さの上限。
pub const MAX_REFUSE_WORDS: usize = 100;
pub const MAX_REFUSE_WORD_CHARS: usize = 100;

/// 答えない質問の決まり（09-qa-chatbot.md「拒否する語」）。
pub async fn refusal(conn: &mut PgConnection) -> Result<super::refuse::Rules> {
    let (words, credit_card, my_number): (Vec<String>, bool, bool) = sqlx::query_as(
        "SELECT refuse_words, refuse_credit_card, refuse_my_number FROM answer_settings",
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(super::refuse::Rules {
        words,
        credit_card,
        my_number,
    })
}

/// 管理APIで返す、答えない質問の決まり。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefusalView {
    pub words: Vec<String>,
    pub credit_card: bool,
    pub my_number: bool,
}

/// 答えない質問の決まりの変更（書いた項目だけを変える。words は書けば一覧ごと置き換える）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RefusalChange {
    #[serde(default)]
    pub words: Option<Vec<String>>,
    #[serde(default)]
    pub credit_card: Option<bool>,
    #[serde(default)]
    pub my_number: Option<bool>,
}

/// 今の語り口。
pub async fn tone(conn: &mut PgConnection) -> Result<Tone> {
    let (politeness, length, enabled, text) = row(conn).await?;
    Ok(Tone {
        politeness: Politeness::parse(&politeness),
        length: Length::parse(&length),
        free_prompt: (enabled && !text.trim().is_empty()).then_some(text),
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FreePrompt {
    pub enabled: bool,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Phrase {
    pub no_information: String,
}

/// 管理APIで返す設定。`phrases` はテンプレートのロケールごと（行がなければ、回答で使う文と同じく英語の行か既定の文）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub politeness: Politeness,
    pub length: Length,
    pub free_prompt: FreePrompt,
    pub phrases: BTreeMap<String, Phrase>,
    pub refusal: RefusalView,
}

pub async fn view(conn: &mut PgConnection, manifest: &Manifest) -> Result<View> {
    let (politeness, length, enabled, text) = row(conn).await?;
    let stored: Vec<(String, String)> =
        sqlx::query_as("SELECT locale, no_information FROM answer_phrases")
            .fetch_all(&mut *conn)
            .await?;
    let lookup = |l: &str| stored.iter().find(|(x, _)| x == l).map(|(_, t)| t.clone());
    let phrases = manifest
        .locales
        .iter()
        .map(|l| {
            let text = lookup(l)
                .or_else(|| lookup("en"))
                .unwrap_or_else(|| super::NO_INFORMATION_FALLBACK.to_owned());
            (
                l.clone(),
                Phrase {
                    no_information: text,
                },
            )
        })
        .collect();
    let r = refusal(conn).await?;
    Ok(View {
        politeness: Politeness::parse(&politeness),
        length: Length::parse(&length),
        free_prompt: FreePrompt { enabled, text },
        phrases,
        refusal: RefusalView {
            words: r.words,
            credit_card: r.credit_card,
            my_number: r.my_number,
        },
    })
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FreePromptChange {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub text: Option<String>,
}

/// 管理APIの変更（書いた項目だけを変える）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Change {
    #[serde(default)]
    pub politeness: Option<Politeness>,
    #[serde(default)]
    pub length: Option<Length>,
    #[serde(default)]
    pub free_prompt: Option<FreePromptChange>,
    #[serde(default)]
    pub phrases: Option<BTreeMap<String, Phrase>>,
    #[serde(default)]
    pub refusal: Option<RefusalChange>,
}

impl Change {
    /// 変えた項目の名前（監査ログに残す。文の中身は残さない）。
    pub fn fields(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.politeness.is_some() {
            out.push("politeness".to_owned());
        }
        if self.length.is_some() {
            out.push("length".to_owned());
        }
        if let Some(f) = &self.free_prompt {
            if f.enabled.is_some() {
                out.push("freePrompt.enabled".to_owned());
            }
            if f.text.is_some() {
                out.push("freePrompt.text".to_owned());
            }
        }
        if let Some(p) = &self.phrases {
            out.extend(p.keys().map(|l| format!("phrases.{l}")));
        }
        if let Some(r) = &self.refusal {
            for (set, name) in [
                (r.words.is_some(), "refusal.words"),
                (r.credit_card.is_some(), "refusal.creditCard"),
                (r.my_number.is_some(), "refusal.myNumber"),
            ] {
                if set {
                    out.push(name.to_owned());
                }
            }
        }
        out
    }
}

/// 改行とタブのほかの制御文字を含むか。
fn has_control(s: &str) -> bool {
    s.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
}

/// 変更を確かめて適用する。問題があれば何も変えずに、その説明の一覧を返す。
pub async fn apply(
    conn: &mut PgConnection,
    manifest: &Manifest,
    change: &Change,
) -> Result<Result<(), Vec<String>>> {
    let (_, _, enabled, text) = row(conn).await?;
    let mut problems = Vec::new();
    let free = change.free_prompt.clone().unwrap_or_default();
    let next_text = free.text.clone().unwrap_or(text);
    let next_enabled = free.enabled.unwrap_or(enabled);
    if next_text.chars().count() > MAX_FREE_PROMPT_CHARS {
        problems.push(format!(
            "freePrompt.text: must be at most {MAX_FREE_PROMPT_CHARS} characters"
        ));
    }
    if has_control(&next_text) {
        problems.push("freePrompt.text: must not contain control characters".to_owned());
    }
    if next_enabled && next_text.trim().is_empty() {
        problems
            .push("freePrompt.text: must not be empty when the free prompt is enabled".to_owned());
    }
    for (locale, phrase) in change.phrases.iter().flatten() {
        let t = &phrase.no_information;
        if !manifest.locales.contains(locale) {
            problems.push(format!(
                "phrases.{locale}: not one of the template's locales"
            ));
        } else if t.trim().is_empty() {
            problems.push(format!("phrases.{locale}.noInformation: must not be empty"));
        } else if t.chars().count() > MAX_PHRASE_CHARS {
            problems.push(format!(
                "phrases.{locale}.noInformation: must be at most {MAX_PHRASE_CHARS} characters"
            ));
        } else if has_control(t) {
            problems.push(format!(
                "phrases.{locale}.noInformation: must not contain control characters"
            ));
        }
    }
    // 拒否する語: 前後の空白を除き、空の語と重複を除く（語そのものは問題の説明に書かない）
    let words: Option<Vec<String>> =
        change
            .refusal
            .as_ref()
            .and_then(|r| r.words.as_ref())
            .map(|ws| {
                let mut out: Vec<String> = Vec::new();
                for w in ws.iter().map(|w| w.trim()).filter(|w| !w.is_empty()) {
                    if !out.iter().any(|x| x == w) {
                        out.push(w.to_owned());
                    }
                }
                out
            });
    if let Some(ws) = &words {
        if ws.len() > MAX_REFUSE_WORDS {
            problems.push(format!("refusal.words: at most {MAX_REFUSE_WORDS} words"));
        }
        for (i, w) in ws.iter().enumerate() {
            if w.chars().count() > MAX_REFUSE_WORD_CHARS {
                problems.push(format!(
                    "refusal.words[{i}]: must be at most {MAX_REFUSE_WORD_CHARS} characters"
                ));
            } else if w.chars().any(char::is_control) {
                problems.push(format!(
                    "refusal.words[{i}]: must not contain control characters"
                ));
            }
        }
    }
    if !problems.is_empty() {
        return Ok(Err(problems));
    }
    let r = change.refusal.clone().unwrap_or_default();
    sqlx::query(
        "UPDATE answer_settings SET
           refuse_words = COALESCE($1, refuse_words),
           refuse_credit_card = COALESCE($2, refuse_credit_card),
           refuse_my_number = COALESCE($3, refuse_my_number)",
    )
    .bind(words)
    .bind(r.credit_card)
    .bind(r.my_number)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE answer_settings SET
           politeness = COALESCE($1, politeness), length = COALESCE($2, length),
           free_prompt_enabled = $3, free_prompt = $4",
    )
    .bind(change.politeness.map(Politeness::as_str))
    .bind(change.length.map(Length::as_str))
    .bind(next_enabled)
    .bind(&next_text)
    .execute(&mut *conn)
    .await?;
    for (locale, phrase) in change.phrases.iter().flatten() {
        sqlx::query(
            "INSERT INTO answer_phrases (locale, no_information) VALUES ($1, $2)
             ON CONFLICT (locale) DO UPDATE SET no_information = EXCLUDED.no_information",
        )
        .bind(locale)
        .bind(phrase.no_information.trim())
        .execute(&mut *conn)
        .await?;
    }
    Ok(Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_changes_with_the_tone() {
        let base = Tone::default();
        let casual = Tone {
            politeness: Politeness::Casual,
            ..Tone::default()
        };
        let free = Tone {
            free_prompt: Some("pirate".into()),
            ..Tone::default()
        };
        assert_ne!(base.fingerprint(), casual.fingerprint());
        assert_ne!(base.fingerprint(), free.fingerprint());
        assert_eq!(base.fingerprint(), Tone::default().fingerprint());
    }

    #[test]
    fn changed_fields_name_only_the_fields() {
        let c: Change = serde_json::from_str(
            r#"{"length":"short","freePrompt":{"text":"secret tone"},"phrases":{"ja":{"noInformation":"なし"}}}"#,
        )
        .unwrap();
        assert_eq!(c.fields(), vec!["length", "freePrompt.text", "phrases.ja"]);
        assert!(serde_json::from_str::<Change>(r#"{"politeness":"rude"}"#).is_err());
        assert!(serde_json::from_str::<Change>(r#"{"other":1}"#).is_err());
    }
}
