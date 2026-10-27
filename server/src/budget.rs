//! LLMの予算（10-llm-and-budget.md「予算」、Q13, Q70, Q138）。M3は「埋め込みの作成」の入力だけ。
//!
//! - 上限は2段。サーバー管理者がワークスペースごとの天井（`workspace_limits`。なければ設定ファイルの既定値、それもなければ 0）を
//!   決め、ワークスペースは重い権限（manageLlm）で、その範囲の上限（`workspace_budgets`）を決める。
//! - 効く上限は、使う瞬間に「ワークスペースの上限と天井の小さいほう」（保存した値をそのまま信用しない。原則3）。
//! - 期間は UTC の暦の月・日。月と日の両方の上限に収まるときだけ呼ぶ。
//! - **呼ぶ前に、多めの見積もり（1文字＝1トークン）を確保する。** 足りなければ呼ばない。確保は行を締めた1つのトランザクションで行い、
//!   同時に呼んでも上限を超えない（INV-BUD-01）。呼んだあと、実際の数（使用量。なければ同じ推定）で精算する。
//! - 精算されなかった確保（呼んでいる途中で落ちたなど）は、使ったものとして残る（上限を超える側に倒れない）。
//! - 予算が足りずに断ったら、期間ごとに1回、ワークスペースの管理者に知らせる（監査ログの知らせる記録）。

use anyhow::Result;
use sqlx::PgPool;
use uuid::Uuid;

use crate::audit;
use crate::config::Config;
use crate::llm::{Embeddings, Llm, LlmError, estimate_tokens};

/// 予算の種類。M3で使うのは埋め込みだけ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Embedding,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Embedding => "embedding",
        }
    }
}

/// 数える向き（埋め込みは入力だけ）。
const INPUT: &str = "input";

/// 月と日の上限（トークン数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pair {
    pub per_month: i64,
    pub per_day: i64,
}

/// ワークスペースの予算の設定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// サーバー管理者の天井。
    pub ceiling: Pair,
    /// ワークスペースが決めた上限（None は上限なし＝天井）。
    pub month: Option<i64>,
    pub day: Option<i64>,
}

impl Limits {
    /// 効く上限（ワークスペースの上限と天井の小さいほう）。
    pub fn effective(&self) -> Pair {
        Pair {
            per_month: self
                .month
                .map_or(self.ceiling.per_month, |m| m.min(self.ceiling.per_month)),
            per_day: self
                .day
                .map_or(self.ceiling.per_day, |d| d.min(self.ceiling.per_day)),
        }
    }
}

fn to_i64(v: Option<u64>) -> i64 {
    v.map_or(0, |v| i64::try_from(v).unwrap_or(i64::MAX))
}

/// サーバー管理者の天井（ワークスペースごとの値、なければ設定ファイルの既定値、それもなければ 0）。
pub async fn ceiling(pool: &PgPool, config: &Config, workspace_id: Uuid) -> Result<Pair> {
    let set: Option<(Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT embedding_tokens_per_month, embedding_tokens_per_day FROM cms.workspace_limits WHERE workspace_id = $1",
    )
    .bind(workspace_id)
    .fetch_optional(pool)
    .await?;
    let (month, day) = set.unwrap_or((None, None));
    let defaults = config.llm.as_ref();
    Ok(Pair {
        per_month: month.unwrap_or_else(|| {
            to_i64(defaults.and_then(|l| l.default_workspace_embedding_tokens_per_month))
        }),
        per_day: day.unwrap_or_else(|| {
            to_i64(defaults.and_then(|l| l.default_workspace_embedding_tokens_per_day))
        }),
    })
}

/// ワークスペースの予算の設定（天井とワークスペースの上限）。
pub async fn limits(
    pool: &PgPool,
    config: &Config,
    workspace_id: Uuid,
    kind: Kind,
) -> Result<Limits> {
    let ceiling = ceiling(pool, config, workspace_id).await?;
    let set: Option<(Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT tokens_per_month, tokens_per_day FROM cms.workspace_budgets
         WHERE workspace_id = $1 AND kind = $2 AND direction = $3",
    )
    .bind(workspace_id)
    .bind(kind.as_str())
    .bind(INPUT)
    .fetch_optional(pool)
    .await?;
    let (month, day) = set.unwrap_or((None, None));
    Ok(Limits {
        ceiling,
        month,
        day,
    })
}

/// ワークスペースの上限を決める（None は上限なし）。呼ぶ側が、権限と天井の範囲を確かめておく。
pub async fn set_limits(
    pool: &PgPool,
    workspace_id: Uuid,
    kind: Kind,
    month: Option<i64>,
    day: Option<i64>,
    actor: Uuid,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO cms.workspace_budgets (workspace_id, kind, direction, tokens_per_month, tokens_per_day, updated_by)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (workspace_id, kind, direction)
         DO UPDATE SET tokens_per_month = $4, tokens_per_day = $5, updated_by = $6, updated_at = now()",
    )
    .bind(workspace_id)
    .bind(kind.as_str())
    .bind(INPUT)
    .bind(month)
    .bind(day)
    .bind(actor)
    .execute(pool)
    .await?;
    resumed(pool, workspace_id, kind).await
}

/// 上限が変わった（ワークスペースの上限か天井）。今の期間の「断った」の印を消す（次に断ったら、また知らせる）。
pub async fn resumed(pool: &PgPool, workspace_id: Uuid, kind: Kind) -> Result<()> {
    sqlx::query(
        "UPDATE cms.budget_usage SET exhausted_at = NULL
         WHERE workspace_id = $1 AND kind = $2 AND exhausted_at IS NOT NULL
           AND ((period = 'month' AND period_start = date_trunc('month', now() AT TIME ZONE 'UTC')::date)
             OR (period = 'day' AND period_start = (now() AT TIME ZONE 'UTC')::date))",
    )
    .bind(workspace_id)
    .bind(kind.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

/// 今の期間の使用量。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Usage {
    pub month: i64,
    pub day: i64,
    /// 今の期間に、予算が足りずに初めて断った時刻（RFC 3339、UTC。上限を変えるまで）。
    pub exhausted_at: Option<String>,
}

pub async fn usage(pool: &PgPool, workspace_id: Uuid, kind: Kind) -> Result<Usage> {
    let rows: Vec<(String, i64, Option<String>)> = sqlx::query_as(
        r#"SELECT period, tokens, to_char(exhausted_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') FROM cms.budget_usage
         WHERE workspace_id = $1 AND kind = $2 AND direction = $3
           AND ((period = 'month' AND period_start = date_trunc('month', now() AT TIME ZONE 'UTC')::date)
             OR (period = 'day' AND period_start = (now() AT TIME ZONE 'UTC')::date))"#,
    )
    .bind(workspace_id)
    .bind(kind.as_str())
    .bind(INPUT)
    .fetch_all(pool)
    .await?;
    let mut out = Usage::default();
    for (period, tokens, exhausted_at) in rows {
        match period.as_str() {
            "month" => out.month = tokens,
            _ => out.day = tokens,
        }
        // 同じ形の文字列なので、文字列の大小が時刻の前後になる
        out.exhausted_at = out.exhausted_at.max(exhausted_at);
    }
    Ok(out)
}

/// 確保した分（精算に使う）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    pub workspace_id: Uuid,
    kind: Kind,
    /// 確保した期間の初日（`YYYY-MM-DD`）。
    month_start: String,
    day_start: String,
    pub tokens: i64,
}

/// 確保の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reserved {
    Ok(Reservation),
    /// 予算が足りない。`first` は、この期間で初めて断ったか（知らせるか）。
    Exhausted {
        first: bool,
    },
}

/// 予算から `tokens` を確保する。月と日の両方の上限に収まるときだけ確保する。
/// 両方の期間の行を締めてから確かめるので、同時に確保しても上限を超えない。
pub async fn reserve(
    pool: &PgPool,
    config: &Config,
    workspace_id: Uuid,
    kind: Kind,
    tokens: i64,
) -> Result<Reserved> {
    let limit = limits(pool, config, workspace_id, kind).await?.effective();
    let mut tx = pool.begin().await?;
    let (month_start, day_start): (String, String) = sqlx::query_as(
        "SELECT date_trunc('month', now() AT TIME ZONE 'UTC')::date::text, (now() AT TIME ZONE 'UTC')::date::text",
    )
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO cms.budget_usage (workspace_id, kind, direction, period, period_start)
         VALUES ($1, $2, $3, 'month', $4::date), ($1, $2, $3, 'day', $5::date)
         ON CONFLICT DO NOTHING",
    )
    .bind(workspace_id)
    .bind(kind.as_str())
    .bind(INPUT)
    .bind(&month_start)
    .bind(&day_start)
    .execute(&mut *tx)
    .await?;
    // 両方の行を、決まった順（月 → 日）で締める
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT period, tokens FROM cms.budget_usage
         WHERE workspace_id = $1 AND kind = $2 AND direction = $3
           AND ((period = 'month' AND period_start = $4::date) OR (period = 'day' AND period_start = $5::date))
         ORDER BY period DESC
         FOR UPDATE",
    )
    .bind(workspace_id)
    .bind(kind.as_str())
    .bind(INPUT)
    .bind(&month_start)
    .bind(&day_start)
    .fetch_all(&mut *tx)
    .await?;
    let used = |p: &str| rows.iter().find(|(r, _)| r == p).map_or(0, |(_, t)| *t);
    let month_ok = used("month").saturating_add(tokens) <= limit.per_month;
    let day_ok = used("day").saturating_add(tokens) <= limit.per_day;
    if month_ok && day_ok {
        sqlx::query(
            "UPDATE cms.budget_usage SET tokens = tokens + $6
             WHERE workspace_id = $1 AND kind = $2 AND direction = $3
               AND ((period = 'month' AND period_start = $4::date) OR (period = 'day' AND period_start = $5::date))",
        )
        .bind(workspace_id)
        .bind(kind.as_str())
        .bind(INPUT)
        .bind(&month_start)
        .bind(&day_start)
        .bind(tokens)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(Reserved::Ok(Reservation {
            workspace_id,
            kind,
            month_start,
            day_start,
            tokens,
        }));
    }
    // 足りない期間の行に「断った」の印を付ける（初めてなら知らせる）
    let marked: Vec<(String,)> = sqlx::query_as(
        "UPDATE cms.budget_usage SET exhausted_at = now()
         WHERE workspace_id = $1 AND kind = $2 AND direction = $3 AND exhausted_at IS NULL
           AND ((period = 'month' AND period_start = $4::date AND $6) OR (period = 'day' AND period_start = $5::date AND $7))
         RETURNING period",
    )
    .bind(workspace_id)
    .bind(kind.as_str())
    .bind(INPUT)
    .bind(&month_start)
    .bind(&day_start)
    .bind(!month_ok)
    .bind(!day_ok)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Reserved::Exhausted {
        first: !marked.is_empty(),
    })
}

/// 精算する。確保した期間の行を、実際の数に直す（余りを返す。確保より多ければ足す）。
pub async fn settle(pool: &PgPool, reservation: &Reservation, actual: i64) -> Result<()> {
    let delta = actual - reservation.tokens;
    if delta == 0 {
        return Ok(());
    }
    sqlx::query(
        "UPDATE cms.budget_usage SET tokens = GREATEST(tokens + $6, 0)
         WHERE workspace_id = $1 AND kind = $2 AND direction = $3
           AND ((period = 'month' AND period_start = $4::date) OR (period = 'day' AND period_start = $5::date))",
    )
    .bind(reservation.workspace_id)
    .bind(reservation.kind.as_str())
    .bind(INPUT)
    .bind(&reservation.month_start)
    .bind(&reservation.day_start)
    .bind(delta)
    .execute(pool)
    .await?;
    Ok(())
}

/// 予算の範囲で埋め込みを作った結果。
#[derive(Debug)]
pub enum Embedded {
    Done(Embeddings),
    /// 予算が足りないので呼ばなかった（次の期間か、上限が上がるまで待つ）。
    OverBudget,
    /// 呼んだが失敗した。
    Failed(LlmError),
}

/// 相手が処理を始める前に失敗した（料金がかかっていない）とみなせるか。
/// 応答を受け取ったあとの失敗や、上限の時間を超えたもの（相手が処理したかわからない）は、確保した分を使ったものとして残す。
fn not_charged(e: LlmError) -> bool {
    use crate::egress::EgressError as E;
    match e {
        LlmError::Status(_) => true,
        LlmError::Egress(e) => matches!(
            e,
            E::InvalidUrl
                | E::SchemeNotAllowed
                | E::InternalDestination
                | E::DnsFailed
                | E::ConnectFailed
                | E::ProxyFailed
                | E::TlsFailed
        ),
        LlmError::InvalidResponse | LlmError::DimensionMismatch => false,
    }
}

/// 予算の範囲で埋め込みを作る。予算が足りなければ呼ばない（INV-BUD-01）。
pub async fn embed(
    pool: &PgPool,
    config: &Config,
    llm: &Llm,
    workspace_id: Uuid,
    inputs: &[String],
) -> Result<Embedded> {
    let estimate = i64::try_from(estimate_tokens(inputs)).unwrap_or(i64::MAX);
    let reservation = match reserve(pool, config, workspace_id, Kind::Embedding, estimate).await? {
        Reserved::Ok(r) => r,
        Reserved::Exhausted { first } => {
            if first {
                audit::record(
                    pool,
                    Some(workspace_id),
                    None,
                    audit::action::BUDGET_EXHAUSTED,
                    &[workspace_id],
                )
                .await?;
            }
            return Ok(Embedded::OverBudget);
        }
    };
    match llm.embed(inputs).await {
        Ok(e) => {
            settle(
                pool,
                &reservation,
                i64::try_from(e.tokens).unwrap_or(i64::MAX),
            )
            .await?;
            Ok(Embedded::Done(e))
        }
        Err(e) => {
            if not_charged(e) {
                settle(pool, &reservation, 0).await?;
            }
            Ok(Embedded::Failed(e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_is_the_smaller_of_workspace_limit_and_ceiling() {
        let ceiling = Pair {
            per_month: 1000,
            per_day: 100,
        };
        let l = |month, day| Limits {
            ceiling,
            month,
            day,
        };
        assert_eq!(l(None, None).effective(), ceiling);
        assert_eq!(
            l(Some(500), Some(500)).effective(),
            Pair {
                per_month: 500,
                per_day: 100
            }
        );
        assert_eq!(
            l(Some(0), None).effective(),
            Pair {
                per_month: 0,
                per_day: 100
            }
        );
    }

    #[test]
    fn charged_errors_keep_the_reservation() {
        use crate::egress::EgressError as E;
        assert!(not_charged(LlmError::Status(500)));
        assert!(not_charged(LlmError::Egress(E::ConnectFailed)));
        assert!(!not_charged(LlmError::Egress(E::Timeout)));
        assert!(!not_charged(LlmError::Egress(E::HttpFailed)));
        assert!(!not_charged(LlmError::InvalidResponse));
    }
}
