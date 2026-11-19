//! 編集者向けのAI支援（10-llm-and-budget.md「編集者向けのAI支援」、Q69, Q70、M4）。
//!
//! - CMSの文脈を使う支援（矛盾の検知、見直しの手がかり、書くべき記事の構成案）と、一般的な執筆支援（構成の確認など。
//!   2026-11-17）を提供する。執筆支援は提案を返すだけで、下書きは変えない（INV-EDIT-02）。
//! - LLM は、ワークスペースの「編集者向けの支援」の予算（`budget::Kind::EditorAssist`）の範囲でだけ呼ぶ。
//!   尽きても公開は妨げない（INV-BUD-02）。公開の処理は、支援の設定・結果・状態を見ない。
//! - 既定は無効。有効にするのは重い権限 manageLlm を持つ人（公開前の下書きが外部のLLMの提供会社に送られるため）。

pub mod alt_text;
pub mod conflicts;
pub mod drafting;
pub mod language;
pub mod llm;
pub mod proofread;
pub mod settings;
pub mod structure;
pub mod tasks;
