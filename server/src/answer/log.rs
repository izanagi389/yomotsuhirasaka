//! 質問ログと「書くべき記事の候補」（09-qa-chatbot.md「質問ログ」、Q14, Q20、INV-LOG-01, 02）。
//!
//! - ワークスペースごとのオプトイン（既定は無効）。無効なら質問文はどこにも保存しない（INV-LOG-01）。
//! - 有効なときに保存するのは、答えられなかった（「情報なし」になった）質問の、マスクした文・ロケール・時刻だけ。
//!   答えた質問（回答キャッシュから返したものを含む）・拒否した質問・エラーは、日ごとの件数だけを残す。
//!   訪問者の識別子やIPアドレスは持たない。
//! - 保存期間を過ぎたものは、常駐の掃除（[`sweep`]）で消す。読むときも期間で絞る（INV-LOG-02）。
//! - 無効にしたら、残っている質問文をすぐに消す（オプトアウトをすぐ反映する）。
//! - 同じ質問（回答キャッシュと同じ正規化）をまとめて、件数の多い順に「書くべき記事の候補」にする。

use std::collections::BTreeMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use super::cache;
use crate::workspace_db::{self, WorkspaceDb};

/// 保存期間の範囲（日）。
pub const RETENTION_DAYS: std::ops::RangeInclusive<i32> = 7..=365;
/// 候補の一覧に出す数の上限。
const MAX_CANDIDATES: i64 = 200;
/// 日ごとの件数を返す日数。
const DAILY_DAYS: i32 = 30;
const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;

/// 回答の結果（日ごとの件数の区分）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Answered,
    NoInformation,
    Refused,
    Error,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Answered => "answered",
            Self::NoInformation => "no_information",
            Self::Refused => "refused",
            Self::Error => "error",
        }
    }
}

/// 回答の結果を記録する（自分の短いトランザクションで）。`masked` は答えられなかった質問のマスクした文で、
/// 質問ログが有効なときだけ保存する（ほかの結果では渡さない）。
pub async fn record(
    pool: &PgPool,
    db: &WorkspaceDb,
    outcome: Outcome,
    locale: &str,
    masked: Option<&str>,
) -> Result<()> {
    let mut tx = workspace_db::begin(pool, db).await?;
    sqlx::query(
        "INSERT INTO question_counts (day, outcome, count) VALUES ((now() AT TIME ZONE 'UTC')::date, $1, 1)
         ON CONFLICT (day, outcome) DO UPDATE SET count = question_counts.count + 1",
    )
    .bind(outcome.as_str())
    .execute(&mut *tx)
    .await?;
    if outcome == Outcome::NoInformation
        && let Some(text) = masked
    {
        // 設定の行を共有ロックで読む。無効にする操作（行の更新と記録の削除を1つのトランザクションで行う）と順番を決め、
        // 無効にした後に質問文が残らないようにする（無効にする操作が先なら、ここは確定を待って無効を読む。こちらが先なら、
        // 無効にする操作がこの記録の確定を待ってから消す）
        let enabled: bool =
            sqlx::query_scalar("SELECT enabled FROM question_log_settings FOR SHARE")
                .fetch_one(&mut *tx)
                .await?;
        if enabled {
            sqlx::query(
                "INSERT INTO question_log (id, key, question, locale) VALUES ($1, $2, $3, $4)",
            )
            .bind(Uuid::new_v4())
            .bind(key(text))
            .bind(text)
            .bind(locale)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

/// 同じ質問をまとめるキー（回答キャッシュと同じ正規化をした文のハッシュ）。
fn key(masked: &str) -> String {
    Sha256::digest(cache::normalize(masked).as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 質問ログの設定。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub enabled: bool,
    pub retention_days: i32,
}

pub async fn settings(conn: &mut PgConnection) -> Result<Settings> {
    let (enabled, retention_days): (bool, i32) =
        sqlx::query_as("SELECT enabled, retention_days FROM question_log_settings")
            .fetch_one(&mut *conn)
            .await?;
    Ok(Settings {
        enabled,
        retention_days,
    })
}

/// 設定の変更（書いた項目だけを変える）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsChange {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub retention_days: Option<i32>,
}

/// 設定を変える。保存期間が範囲の外なら、何も変えずに Err。無効にしたら、残っている質問文をすぐに消す。
pub async fn change_settings(
    conn: &mut PgConnection,
    change: &SettingsChange,
) -> Result<Result<Settings, String>> {
    if change
        .retention_days
        .is_some_and(|d| !RETENTION_DAYS.contains(&d))
    {
        return Ok(Err(format!(
            "retentionDays: must be between {} and {}",
            RETENTION_DAYS.start(),
            RETENTION_DAYS.end()
        )));
    }
    sqlx::query(
        "UPDATE question_log_settings SET enabled = COALESCE($1, enabled),
           retention_days = COALESCE($2, retention_days)",
    )
    .bind(change.enabled)
    .bind(change.retention_days)
    .execute(&mut *conn)
    .await?;
    let now = settings(conn).await?;
    if !now.enabled {
        sqlx::query("DELETE FROM question_log")
            .execute(&mut *conn)
            .await?;
        // 質問文から作った構成案も消す
        crate::assist::tasks::forget_outlines(conn, None).await?;
    }
    Ok(Ok(now))
}

/// 「書くべき記事の候補」（同じ質問をまとめたもの）。`id` は削除に使う。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub id: String,
    /// いちばん新しい質問のマスクした文。
    pub question: String,
    pub locale: String,
    pub count: i64,
    pub last_asked_at: String,
}

/// 日ごとの件数（UTC の日）。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Daily {
    pub date: String,
    pub answered: i64,
    pub no_information: i64,
    pub refused: i64,
    pub error: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    #[serde(flatten)]
    pub settings: Settings,
    pub candidates: Vec<Candidate>,
    /// 直近の日ごとの件数（新しい日が先）。
    pub daily: Vec<Daily>,
}

pub async fn view(conn: &mut PgConnection) -> Result<View> {
    let settings = settings(conn).await?;
    let rows: Vec<(String, String, String, i64, String)> = sqlx::query_as(crate::sql!(
        "SELECT key, (array_agg(question ORDER BY asked_at DESC))[1], (array_agg(locale ORDER BY asked_at DESC))[1],
                count(*), to_char(max(asked_at) AT TIME ZONE 'UTC', {RFC3339})
         FROM question_log
         WHERE asked_at > now() - make_interval(days => $1)
         GROUP BY key
         ORDER BY count(*) DESC, max(asked_at) DESC
         LIMIT $2"
    ))
    .bind(settings.retention_days)
    .bind(MAX_CANDIDATES)
    .fetch_all(&mut *conn)
    .await?;
    let candidates = rows
        .into_iter()
        .map(|(id, question, locale, count, last_asked_at)| Candidate {
            id,
            question,
            locale,
            count,
            last_asked_at,
        })
        .collect();
    let counts: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT to_char(day, 'YYYY-MM-DD'), outcome, count FROM question_counts
         WHERE day > (now() AT TIME ZONE 'UTC')::date - $1",
    )
    .bind(DAILY_DAYS)
    .fetch_all(&mut *conn)
    .await?;
    let mut days: BTreeMap<String, Daily> = BTreeMap::new();
    for (date, outcome, count) in counts {
        let d = days.entry(date.clone()).or_insert_with(|| Daily {
            date,
            ..Daily::default()
        });
        match outcome.as_str() {
            "answered" => d.answered = count,
            "no_information" => d.no_information = count,
            "refused" => d.refused = count,
            _ => d.error = count,
        }
    }
    Ok(View {
        settings,
        candidates,
        daily: days.into_values().rev().collect(),
    })
}

/// 候補（同じ質問の記録すべて）を消す。消したものがなければ false。
pub async fn delete_candidate(conn: &mut PgConnection, id: &str) -> Result<bool> {
    let deleted = sqlx::query("DELETE FROM question_log WHERE key = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?
        .rows_affected()
        > 0;
    // 候補から作った構成案も消す
    crate::assist::tasks::forget_outlines(conn, Some(id)).await?;
    Ok(deleted)
}

/// 保存期間を過ぎた質問ログを、すべてのワークスペースで消す（常駐の処理から1時間ごとに呼ぶ）。
pub async fn sweep(pool: &PgPool) -> Result<u64> {
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM cms.workspaces ORDER BY created_at")
        .fetch_all(pool)
        .await?;
    let mut removed = 0;
    for id in ids {
        let Some(db) = workspace_db::lookup(pool, id).await? else {
            continue;
        };
        let mut tx = workspace_db::begin(pool, &db).await?;
        // 保存期間を過ぎたもの。無効のワークスペースに残ったものも消す（念のため）
        removed += sqlx::query(
            "DELETE FROM question_log
             WHERE asked_at <= now() - make_interval(days => (SELECT retention_days FROM question_log_settings))
                OR NOT (SELECT enabled FROM question_log_settings)",
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_questions_share_a_key() {
        assert_eq!(key("ＡＢＣ とは"), key("abc  とは"));
        assert_ne!(key("abc とは"), key("abd とは"));
    }
}
