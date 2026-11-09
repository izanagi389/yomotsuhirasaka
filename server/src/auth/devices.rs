//! 新しい端末の判定と、ログインの履歴（13-security.md「本人への通知」、Q171, Q175, Q176, Q181）。
//!
//! - ログインに成功した端末に、長期のCookie（ランダムなID）を置く。DBにはハッシュだけを保存する。
//! - このCookieは、新しい端末の通知と、ログインの失敗を端末ごとに数えることに使う。多要素認証の省略などには使わない（Q181）。
//!   知っている端末の失敗はアカウントの待ち時間に入れず、アカウントの待ち時間にも止められない（`throttle`）。
//! - ログインの履歴には、IPアドレス、ブラウザの種類（決まった名前の1つ）、国（GeoIPのデータがある場合だけ）を残す。
//! - 知っている端末でも、前回と違う国からのログインなら知らせる（国のデータがある場合だけ。Q175）。
//!   User-Agent はそのまま保存しない。保存期間を過ぎた履歴は、定期的に削除する（INV-AUD-07）。
//!   一覧を返すときも、保存期間の内側だけを返す（削除が遅れても見せない）。

use std::net::IpAddr;

use anyhow::Result;
use sqlx::PgPool;
use uuid::Uuid;

use super::token;

/// 端末のCookieの名前。
pub const DEVICE_COOKIE: &str = "__Host-yomi_device";
/// 端末のCookieの有効期間（秒）。
pub const DEVICE_COOKIE_MAX_AGE: u32 = 365 * 24 * 60 * 60;
/// この日数のあいだログインのなかった端末の記録は消す（Cookieの有効期間より少し長く）。
const DEVICE_RETENTION_DAYS: i32 = 400;
/// 一覧で返す履歴の最大件数。
const LIST_LIMIT: i64 = 200;

const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;

/// 本人に知らせること。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    /// 新しい端末からのログイン（初めての端末は除く）。
    NewDevice,
    /// 知っている端末だが、前回と違う国からのログイン（国のデータがある場合だけ。Q175）。
    NewCountry,
}

/// ログインの記録の結果。
pub struct Recorded {
    /// 端末のCookieに入れる値（受け取った値か、新しく作った値）。
    pub device_token: String,
    /// この利用者にとって新しい端末か。
    pub new_device: bool,
    /// 本人に知らせることがあれば、その種類。
    pub notice: Option<Notice>,
}

/// 受け取った端末のCookieが、こちらで作った形（43文字のURL-safe base64）か。
fn well_formed(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// この利用者の知っている端末か（Cookie の形が正しく、記録があるとき）。あれば端末のハッシュと、待ち時間中かを返す。
pub async fn known_device(
    pool: &PgPool,
    user_id: Uuid,
    presented: Option<&str>,
) -> Result<Option<(Vec<u8>, bool)>> {
    let Some(value) = presented.filter(|v| well_formed(v)) else {
        return Ok(None);
    };
    let hash = token::digest(value);
    let blocked: Option<bool> = sqlx::query_scalar(
        "SELECT coalesce(login_blocked_until > now(), false) FROM yomi.known_devices
         WHERE user_id = $1 AND device_hash = $2",
    )
    .bind(user_id)
    .bind(&hash)
    .fetch_optional(pool)
    .await?;
    Ok(blocked.map(|b| (hash, b)))
}

/// User-Agent を、ブラウザの種類の決まった名前の1つにまとめる。
pub fn browser_family(user_agent: Option<&str>) -> &'static str {
    let Some(ua) = user_agent else {
        return "Unknown";
    };
    let has = |s: &str| ua.contains(s);
    if has("Edg/") || has("EdgA/") || has("EdgiOS/") {
        "Edge"
    } else if has("OPR/") || has("Opera") {
        "Opera"
    } else if has("SamsungBrowser/") {
        "Samsung Internet"
    } else if has("Firefox/") || has("FxiOS/") {
        "Firefox"
    } else if has("Chrome/") || has("CriOS/") || has("Chromium/") {
        "Chrome"
    } else if has("Safari/") {
        "Safari"
    } else {
        "Other"
    }
}

/// ログインの成功を記録する（端末と国の判定、履歴）。`country` は国のデータがある場合だけ。
pub async fn record_login(
    pool: &PgPool,
    user_id: Uuid,
    presented_device: Option<&str>,
    ip: IpAddr,
    user_agent: Option<&str>,
    country: Option<&str>,
) -> Result<Recorded> {
    let device_token = presented_device
        .filter(|v| well_formed(v))
        .map_or_else(token::generate, str::to_owned);
    let hash = token::digest(&device_token);

    let mut tx = pool.begin().await?;
    let known = sqlx::query(
        "UPDATE yomi.known_devices SET last_seen_at = now() WHERE user_id = $1 AND device_hash = $2",
    )
    .bind(user_id)
    .bind(&hash)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    let mut notice = None;
    if !known {
        let had_any: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM yomi.known_devices WHERE user_id = $1)",
        )
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO yomi.known_devices (user_id, device_hash) VALUES ($1, $2)
             ON CONFLICT (user_id, device_hash) DO UPDATE SET last_seen_at = now()",
        )
        .bind(user_id)
        .bind(&hash)
        .execute(&mut *tx)
        .await?;
        // 初めての端末（招待を受けた直後など）は、知らせても本人の操作に決まっているので知らせない
        if had_any {
            notice = Some(Notice::NewDevice);
        }
    }
    if notice.is_none()
        && let Some(country) = country
    {
        // 国の変化: 国の分かっている直前のログインと比べる（分からなければ知らせない）
        let previous: Option<String> = sqlx::query_scalar(
            "SELECT country FROM yomi.login_history
             WHERE user_id = $1 AND country IS NOT NULL
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        if previous.is_some_and(|p| p != country) {
            notice = Some(Notice::NewCountry);
        }
    }
    sqlx::query(
        "INSERT INTO yomi.login_history (id, user_id, ip, browser, country, new_device)
         VALUES ($1, $2, $3::inet, $4, $5, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(ip.to_canonical().to_string())
    .bind(browser_family(user_agent))
    .bind(country)
    .bind(!known)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Recorded {
        device_token,
        new_device: !known,
        notice,
    })
}

/// ログインの履歴の1件。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub at: String,
    pub ip: String,
    pub browser: String,
    pub country: Option<String>,
    pub new_device: bool,
}

type EntryRow = (String, String, String, Option<String>, bool);

/// 本人のログインの履歴（新しい順。保存期間の内側だけ）。
pub async fn list(pool: &PgPool, user_id: Uuid, retention_seconds: u32) -> Result<Vec<Entry>> {
    let rows: Vec<EntryRow> = sqlx::query_as(crate::sql!(
        "SELECT to_char(created_at AT TIME ZONE 'UTC', {RFC3339}), host(ip), browser, country, new_device
         FROM yomi.login_history
         WHERE user_id = $1 AND created_at >= now() - make_interval(secs => $2)
         ORDER BY created_at DESC
         LIMIT $3"
    ))
    .bind(user_id)
    .bind(f64::from(retention_seconds))
    .bind(LIST_LIMIT)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(at, ip, browser, country, new_device)| Entry {
            at,
            ip,
            browser,
            country,
            new_device,
        })
        .collect())
}

/// 保存期間を過ぎた履歴と、長く使われていない端末の記録を消す。消した履歴の件数を返す。
pub async fn sweep(pool: &PgPool, retention_seconds: u32) -> Result<u64> {
    let removed = sqlx::query(
        "DELETE FROM yomi.login_history WHERE created_at < now() - make_interval(secs => $1)",
    )
    .bind(f64::from(retention_seconds))
    .execute(pool)
    .await?
    .rows_affected();
    sqlx::query(
        "DELETE FROM yomi.known_devices WHERE last_seen_at < now() - make_interval(days => $1)",
    )
    .bind(DEVICE_RETENTION_DAYS)
    .execute(pool)
    .await?;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_families() {
        let cases = [
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36",
                "Chrome",
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36 Edg/129.0.0.0",
                "Edge",
            ),
            (
                "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0",
                "Firefox",
            ),
            (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15",
                "Safari",
            ),
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/129.0 Mobile/15E148 Safari/604.1",
                "Chrome",
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36 OPR/114.0.0.0",
                "Opera",
            ),
            ("curl/8.5.0", "Other"),
        ];
        for (ua, expected) in cases {
            assert_eq!(browser_family(Some(ua)), expected, "{ua}");
        }
        assert_eq!(browser_family(None), "Unknown");
    }

    #[test]
    fn device_cookie_format() {
        assert!(well_formed(&token::generate()));
        assert!(!well_formed("short"));
        assert!(!well_formed(&"a".repeat(42)));
        assert!(!well_formed(&format!("{};", "a".repeat(42))));
    }
}
