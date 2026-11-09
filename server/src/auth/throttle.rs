//! ログインの回数制限（Q105）。
//!
//! - アカウント単位: 失敗が続くと、次に試せるまでの待ち時間を段階的に延ばす（上限15分。完全なロックにはしない）。
//!   状態はDB（`yomi.users`）に持つ。待ち時間中は、正しいパスワードでもログインできない。
//! - 知っている端末からのログインは、端末ごとに同じように数える（`yomi.known_devices`）。アカウントの待ち時間の対象にせず、
//!   アカウントの失敗にも数えない。パスワードを知らない人が失敗を送り続けても、本人を締め出せないようにするため。
//! - 本人確認の2段目（多要素認証のコードと再認証）の失敗は、パスワードの失敗とは別に数える。
//!   パスワードの成功で消えると、パスワードを知っている人がコードを総当たりできるため。
//!   また、パスワードを知らない人の失敗で、ログイン中の人の再認証が止まらないようにするため。
//! - IPアドレス単位: 1分あたりの試行の回数を制限する。状態はプロセスのメモリに持つ。

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Result;
use sqlx::PgPool;
use uuid::Uuid;

/// 待ち時間なしで許す、連続した失敗の回数。
pub const FREE_FAILURES: i32 = 5;
/// 最初の待ち時間（秒）。以後、失敗のたびに2倍にする。
pub const BASE_DELAY_SECONDS: f64 = 15.0;
/// 待ち時間の上限（秒）。
pub const MAX_DELAY_SECONDS: f64 = 15.0 * 60.0;

/// 失敗を記録し、必要なら待ち時間を設定する（1つの文で行い、同時の失敗を取りこぼさない）。
/// 待ち時間は、FREE_FAILURES 回目の失敗で BASE_DELAY_SECONDS、以後2倍ずつ、上限 MAX_DELAY_SECONDS。
pub async fn record_failure(pool: &PgPool, user_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE yomi.users SET
            failed_login_count = failed_login_count + 1,
            login_blocked_until = CASE
                WHEN failed_login_count + 1 >= $2
                THEN now() + make_interval(secs => LEAST($4, $3 * power(2::float8, LEAST(failed_login_count + 1 - $2, 30)::float8)))
                ELSE login_blocked_until
            END
         WHERE id = $1",
    )
    .bind(user_id)
    .bind(FREE_FAILURES)
    .bind(BASE_DELAY_SECONDS)
    .bind(MAX_DELAY_SECONDS)
    .execute(pool)
    .await?;
    Ok(())
}

/// 成功したら、失敗の記録を消す。
pub async fn record_success(pool: &PgPool, user_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE yomi.users SET failed_login_count = 0, login_blocked_until = NULL
         WHERE id = $1 AND (failed_login_count <> 0 OR login_blocked_until IS NOT NULL)",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// 知っている端末からの失敗を記録し、必要なら待ち時間を設定する（数え方はアカウントと同じ）。
pub async fn record_device_failure(pool: &PgPool, user_id: Uuid, device_hash: &[u8]) -> Result<()> {
    sqlx::query(
        "UPDATE yomi.known_devices SET
            failed_login_count = failed_login_count + 1,
            login_blocked_until = CASE
                WHEN failed_login_count + 1 >= $3
                THEN now() + make_interval(secs => LEAST($5, $4 * power(2::float8, LEAST(failed_login_count + 1 - $3, 30)::float8)))
                ELSE login_blocked_until
            END
         WHERE user_id = $1 AND device_hash = $2",
    )
    .bind(user_id)
    .bind(device_hash)
    .bind(FREE_FAILURES)
    .bind(BASE_DELAY_SECONDS)
    .bind(MAX_DELAY_SECONDS)
    .execute(pool)
    .await?;
    Ok(())
}

/// 知っている端末からパスワードが通ったら、その端末の失敗の記録を消す（アカウントの記録は消さない）。
pub async fn record_device_success(pool: &PgPool, user_id: Uuid, device_hash: &[u8]) -> Result<()> {
    sqlx::query(
        "UPDATE yomi.known_devices SET failed_login_count = 0, login_blocked_until = NULL
         WHERE user_id = $1 AND device_hash = $2
           AND (failed_login_count <> 0 OR login_blocked_until IS NOT NULL)",
    )
    .bind(user_id)
    .bind(device_hash)
    .execute(pool)
    .await?;
    Ok(())
}

/// 2段目の失敗を記録し、必要なら待ち時間を設定する（数え方はパスワードと同じ）。
pub async fn record_second_factor_failure(pool: &PgPool, user_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE yomi.users SET
            second_factor_failed_count = second_factor_failed_count + 1,
            second_factor_blocked_until = CASE
                WHEN second_factor_failed_count + 1 >= $2
                THEN now() + make_interval(secs => LEAST($4, $3 * power(2::float8, LEAST(second_factor_failed_count + 1 - $2, 30)::float8)))
                ELSE second_factor_blocked_until
            END
         WHERE id = $1",
    )
    .bind(user_id)
    .bind(FREE_FAILURES)
    .bind(BASE_DELAY_SECONDS)
    .bind(MAX_DELAY_SECONDS)
    .execute(pool)
    .await?;
    Ok(())
}

/// 2段目に成功したら、2段目の失敗の記録を消す。
pub async fn record_second_factor_success(pool: &PgPool, user_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE yomi.users SET second_factor_failed_count = 0, second_factor_blocked_until = NULL
         WHERE id = $1 AND (second_factor_failed_count <> 0 OR second_factor_blocked_until IS NOT NULL)",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// IPアドレスごとの、1分あたりの回数の制限。
pub struct IpLimiter {
    per_window: u32,
    window: Duration,
    state: Mutex<HashMap<IpAddr, (Instant, u32)>>,
}

/// メモリに持つ接続元の数の上限の目安。超えたら、期限の切れた記録を捨てる。
const PRUNE_THRESHOLD: usize = 50_000;

impl IpLimiter {
    pub fn per_minute(limit: u32) -> Self {
        Self {
            per_window: limit,
            window: Duration::from_secs(60),
            state: Mutex::new(HashMap::new()),
        }
    }

    /// すでに上限に達しているか（数えない）。
    pub fn exceeded(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.get(&limit_key(ip)).is_some_and(|(start, count)| {
            now.duration_since(*start) < self.window && *count >= self.per_window
        })
    }

    /// 1回の試行を数える。上限を超えていれば、次に試せるまでの秒数を返す。
    pub fn check(&self, ip: IpAddr) -> Result<(), u64> {
        let key = limit_key(ip);
        let now = Instant::now();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.len() >= PRUNE_THRESHOLD {
            let window = self.window;
            state.retain(|_, (start, _)| now.duration_since(*start) < window);
        }
        let entry = state.entry(key).or_insert((now, 0));
        if now.duration_since(entry.0) >= self.window {
            *entry = (now, 0);
        }
        if entry.1 >= self.per_window {
            let wait = self.window.saturating_sub(now.duration_since(entry.0));
            return Err(wait.as_secs().max(1));
        }
        entry.1 += 1;
        Ok(())
    }
}

/// 制限の単位。IPv6は /64 ごとにまとめる（1人が多数のアドレスを使えるため）。
fn limit_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let s = v6.segments();
                IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_per_address() {
        let l = IpLimiter::per_minute(3);
        let a: IpAddr = "198.18.0.1".parse().unwrap();
        let b: IpAddr = "198.18.0.2".parse().unwrap();
        for _ in 0..3 {
            assert!(l.check(a).is_ok());
        }
        let wait = l.check(a).unwrap_err();
        assert!((1..=60).contains(&wait));
        assert!(l.check(b).is_ok());
    }

    #[test]
    fn ipv6_is_grouped_by_64() {
        let l = IpLimiter::per_minute(1);
        assert!(l.check("2001:db8:1:2::1".parse().unwrap()).is_ok());
        assert!(l.check("2001:db8:1:2:ffff::9".parse().unwrap()).is_err());
        assert!(l.check("2001:db8:1:3::1".parse().unwrap()).is_ok());
        // IPv4射影アドレスはIPv4として数える
        assert!(l.check("198.18.0.9".parse().unwrap()).is_ok());
        assert!(l.check("::ffff:198.18.0.9".parse().unwrap()).is_err());
    }
}
