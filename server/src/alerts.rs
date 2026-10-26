//! 管理者への通知のうち、急増の検知（13-security.md「管理者への通知」、Q174、計画の15番）。
//!
//! - 数えるもの: ログインの失敗（パスワードと2段目）と、回数制限への到達（429）。
//! - 攻撃のたびに DB へ書かないよう、プロセスのメモリでまとめて数え、10秒ごとに1分単位の件数として DB（`cms.alert_events`）に足す。
//!   サーバーが複数でも、件数は DB で合わせて数える。
//! - 直近 window_seconds の件数がしきい値以上なら、インスタンス単位の通知（`alert.*`）を監査ログに残す。同じ種類は
//!   cooldown_seconds に1回まで（複数のサーバーで二重に残さないよう、アドバイザリーロックの中で判断する）。
//! - 通知の詳細には、件数・期間と、件数の多い順に最大10件ずつのIPアドレス・利用者のID・メールアドレスを入れる。
//!   メールアドレスは存在するアカウントのものだけ（入力された任意の文字列は流さない）。インスタンス単位なので、
//!   サーバー管理者とインスタンス全体の書き出し先にだけ出る（ワークスペースをまたぐ情報のため。Q101）。
//! - 1日より古い件数は消す。

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use anyhow::Result;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::audit::{self, action};
use crate::config::AlertsConfig;

/// 数える種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    LoginFailure,
    RateLimited,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::LoginFailure => "login_failure",
            Kind::RateLimited => "rate_limited",
        }
    }
    fn action(self) -> &'static str {
        match self {
            Kind::LoginFailure => action::ALERT_LOGIN_FAILURES,
            Kind::RateLimited => action::ALERT_RATE_LIMITED,
        }
    }
    fn threshold(self, c: &AlertsConfig) -> i64 {
        match self {
            Kind::LoginFailure => c.login_failures,
            Kind::RateLimited => c.rate_limited,
        }
    }
}

const KINDS: [Kind; 2] = [Kind::LoginFailure, Kind::RateLimited];
/// メモリにためる組の数の上限（超えたら、IPアドレスと利用者を分けずに数える）。
const MAX_KEYS: usize = 10_000;
/// DB に書いて判断する間隔。
const FLUSH: Duration = Duration::from_secs(10);
/// 詳細に入れる件数。
const TOP: i64 = 10;

type Key = (Kind, Option<IpAddr>, Option<Uuid>);

static PENDING: LazyLock<Mutex<HashMap<Key, i64>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// 1件数える（すぐに戻る。DB には書かない）。
pub fn note(kind: Kind, ip: Option<IpAddr>, user: Option<Uuid>) {
    let Ok(mut pending) = PENDING.lock() else {
        return;
    };
    let key = if pending.len() >= MAX_KEYS && !pending.contains_key(&(kind, ip, user)) {
        (kind, None, None)
    } else {
        (kind, ip, user)
    };
    *pending.entry(key).or_insert(0) += 1;
}

/// たまった件数を DB に足す。
async fn flush(pool: &PgPool) -> Result<()> {
    let drained: Vec<(Key, i64)> = match PENDING.lock() {
        Ok(mut p) => p.drain().collect(),
        Err(_) => return Ok(()),
    };
    for ((kind, ip, user), count) in drained {
        sqlx::query(
            "INSERT INTO cms.alert_events (kind, bucket_at, ip, user_id, count)
             VALUES ($1, date_trunc('minute', now()), $2, $3, $4)
             ON CONFLICT (kind, bucket_at, coalesce(ip, ''), coalesce(user_id, '00000000-0000-0000-0000-000000000000'))
             DO UPDATE SET count = cms.alert_events.count + EXCLUDED.count",
        )
        .bind(kind.name())
        .bind(ip.map(|i| i.to_string()))
        .bind(user)
        .bind(count)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// しきい値を超えていれば、通知を残す。
async fn check(pool: &PgPool, config: &AlertsConfig, kind: Kind) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('cms:alerts:' || $1, 0))")
        .bind(kind.name())
        .execute(&mut *tx)
        .await?;
    let window = config.window_seconds as f64;
    let total: i64 = sqlx::query_scalar(
        "SELECT coalesce(sum(count), 0)::bigint FROM cms.alert_events
         WHERE kind = $1 AND bucket_at > now() - make_interval(secs => $2)",
    )
    .bind(kind.name())
    .bind(window)
    .fetch_one(&mut *tx)
    .await?;
    if total < kind.threshold(config) {
        return Ok(());
    }
    let recent: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM cms.audit_log WHERE workspace_id IS NULL AND action = $1
                        AND created_at > now() - make_interval(secs => $2))",
    )
    .bind(kind.action())
    .bind(config.cooldown_seconds as f64)
    .fetch_one(&mut *tx)
    .await?;
    if recent {
        return Ok(());
    }
    let ips: Vec<(String, i64)> = sqlx::query_as(
        "SELECT ip, sum(count)::bigint AS n FROM cms.alert_events
         WHERE kind = $1 AND bucket_at > now() - make_interval(secs => $2) AND ip IS NOT NULL
         GROUP BY ip ORDER BY n DESC, ip LIMIT $3",
    )
    .bind(kind.name())
    .bind(window)
    .bind(TOP)
    .fetch_all(&mut *tx)
    .await?;
    let users: Vec<(Uuid, Option<String>, i64)> = sqlx::query_as(
        "SELECT e.user_id, u.email, sum(e.count)::bigint AS n FROM cms.alert_events e
         LEFT JOIN cms.users u ON u.id = e.user_id
         WHERE e.kind = $1 AND e.bucket_at > now() - make_interval(secs => $2) AND e.user_id IS NOT NULL
         GROUP BY e.user_id, u.email ORDER BY n DESC, e.user_id LIMIT $3",
    )
    .bind(kind.name())
    .bind(window)
    .bind(TOP)
    .fetch_all(&mut *tx)
    .await?;
    let details = json!({
        "count": total,
        "windowSeconds": config.window_seconds,
        "topIps": ips.iter().map(|(ip, n)| json!({ "ip": ip, "count": n })).collect::<Vec<_>>(),
        "topUsers": users.iter().map(|(id, email, n)| json!({ "userId": id, "email": email, "count": n })).collect::<Vec<_>>(),
    });
    let targets: Vec<Uuid> = users.iter().map(|(id, _, _)| *id).collect();
    audit::record_full(
        &mut *tx,
        None,
        None,
        kind.action(),
        &targets,
        true,
        Some(&details),
    )
    .await?;
    tx.commit().await?;
    tracing::warn!(
        kind = kind.name(),
        count = total,
        "admin alert: spike detected"
    );
    Ok(())
}

/// 急増の検知の常駐の処理を始める。
pub fn spawn(pool: PgPool, config: AlertsConfig) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(FLUSH);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if let Err(e) = flush(&pool).await {
                tracing::error!("admin alerts: cannot record counts: {e:#}");
                continue;
            }
            for kind in KINDS {
                if let Err(e) = check(&pool, &config, kind).await {
                    tracing::error!("admin alerts: {e:#}");
                }
            }
            if let Err(e) = sqlx::query(
                "DELETE FROM cms.alert_events WHERE bucket_at < now() - interval '1 day'",
            )
            .execute(&pool)
            .await
            {
                tracing::error!("admin alerts: cannot remove old counts: {e:#}");
            }
        }
    })
}
