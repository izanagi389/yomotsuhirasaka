//! 利用者に向けた文言の言語（Q198）。管理画面の文言は翻訳ファイルに分ける（CLAUDE.md）。ここではサーバーが送るメールの言語だけを扱う。

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// 対応する言語。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Locale {
    /// 英語（インスタンスの既定の言語の初期値）。
    #[default]
    En,
    Ja,
}

impl Locale {
    pub fn as_str(self) -> &'static str {
        match self {
            Locale::En => "en",
            Locale::Ja => "ja",
        }
    }
}

impl fmt::Display for Locale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Locale {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "en" => Ok(Locale::En),
            "ja" => Ok(Locale::Ja),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for l in [Locale::En, Locale::Ja] {
            assert_eq!(l.as_str().parse::<Locale>(), Ok(l));
        }
        assert!("fr".parse::<Locale>().is_err());
        assert_eq!(Locale::default(), Locale::En);
    }
}
