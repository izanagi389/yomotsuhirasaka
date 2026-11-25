//! 日付と日時のフィールドの値の検査と、そろえた形への変換（02-schema-and-delivery-api.md「フィールドの型」、2026-11-25）。
//!
//! - 日付は `YYYY-MM-DD`（時差を持たない）。
//! - 日時は時差の付いた RFC 3339（`2026-11-25T10:00:00+09:00`・`...Z`）。UTC の `YYYY-MM-DDTHH:MM:SSZ` にそろえる。
//!   秒より細かい部分は持たない。`.000` のような0だけの端数は受け付け、0でない端数は値を黙って切り捨てないために拒否する。
//!
//! 依存を足さないために、受け付ける書式を絞って自前で読む（暗号処理ではない）。

/// 受け付ける年の範囲（4桁で書け、文字列の順と時刻の順が一致する範囲）。
const YEARS: std::ops::RangeInclusive<i64> = 1..=9999;

const SECONDS_PER_DAY: i64 = 86_400;

/// `YYYY-MM-DD` を読む。実在する日付だけを受け付ける。
fn parse_ymd(s: &[u8]) -> Option<(i64, i64, i64)> {
    if s.len() != 10 || s[4] != b'-' || s[7] != b'-' {
        return None;
    }
    let year = digits(&s[0..4])?;
    let month = digits(&s[5..7])?;
    let day = digits(&s[8..10])?;
    (YEARS.contains(&year)
        && (1..=12).contains(&month)
        && (1..=days_in_month(year, month)).contains(&day))
    .then_some((year, month, day))
}

/// ASCII の数字だけの並びを数にする。
fn digits(s: &[u8]) -> Option<i64> {
    if s.is_empty() || !s.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(s.iter().fold(0, |n, d| n * 10 + i64::from(d - b'0')))
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// 1970-01-01 からの日数（グレゴリオ暦。Howard Hinnant の `days_from_civil`）。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// [`days_from_civil`] の逆。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// 日付のフィールドの値として正しいか（`YYYY-MM-DD` の実在する日付）。
pub fn valid_date(s: &str) -> bool {
    parse_ymd(s.as_bytes()).is_some()
}

/// `HH:MM:SS` と、0だけの端数を読み、その日の0時からの秒数にする。残り（時差の部分）を返す。
fn parse_time(s: &[u8]) -> Option<(i64, &[u8])> {
    if s.len() < 8 || s[2] != b':' || s[5] != b':' {
        return None;
    }
    let (hour, minute, second) = (digits(&s[0..2])?, digits(&s[3..5])?, digits(&s[6..8])?);
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let mut rest = &s[8..];
    if let Some(fraction) = rest.strip_prefix(b".") {
        let len = fraction.iter().take_while(|b| b.is_ascii_digit()).count();
        if len == 0 || fraction[..len].iter().any(|b| *b != b'0') {
            return None;
        }
        rest = &fraction[len..];
    }
    Some((hour * 3600 + minute * 60 + second, rest))
}

/// 時差（`Z`・`+HH:MM`・`-HH:MM`）を秒数にする。
fn parse_offset(s: &[u8]) -> Option<i64> {
    match s {
        b"Z" => Some(0),
        [sign @ (b'+' | b'-'), h1, h2, b':', m1, m2] => {
            let hours = digits(&[*h1, *h2])?;
            let minutes = digits(&[*m1, *m2])?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            let seconds = hours * 3600 + minutes * 60;
            Some(if *sign == b'-' { -seconds } else { seconds })
        }
        _ => None,
    }
}

/// 日時のフィールドの値を読み、UTC の `YYYY-MM-DDTHH:MM:SSZ` にそろえる。正しくなければ `None`。
pub fn normalize_datetime(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    if bytes.len() < 11 || bytes[10] != b'T' {
        return None;
    }
    let (year, month, day) = parse_ymd(&bytes[..10])?;
    let (seconds_of_day, rest) = parse_time(&bytes[11..])?;
    let offset = parse_offset(rest)?;
    let utc = days_from_civil(year, month, day) * SECONDS_PER_DAY + seconds_of_day - offset;
    let (days, secs) = (
        utc.div_euclid(SECONDS_PER_DAY),
        utc.rem_euclid(SECONDS_PER_DAY),
    );
    let (y, m, d) = civil_from_days(days);
    if !YEARS.contains(&y) {
        return None;
    }
    Some(format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        for ok in [
            "2026-02-28",
            "2024-02-29",
            "2000-02-29",
            "0001-01-01",
            "9999-12-31",
        ] {
            assert!(valid_date(ok), "{ok}");
        }
        for bad in [
            "2026-13-01",
            "2026-02-30",
            "2026-02-29",
            "1900-02-29",
            "2026/11/25",
            "2026-11-25T10:00:00Z",
            "26-11-25",
            "2026-1-05",
            "0000-01-01",
            "+026-11-25",
            "２０２６-11-25",
            "",
        ] {
            assert!(!valid_date(bad), "{bad}");
        }
    }

    #[test]
    fn datetimes_are_normalized_to_utc_seconds() {
        for (input, want) in [
            ("2026-11-25T10:00:00+09:00", "2026-11-25T01:00:00Z"),
            ("2026-11-25T01:00:00Z", "2026-11-25T01:00:00Z"),
            ("2026-11-25T01:00:00.000Z", "2026-11-25T01:00:00Z"),
            ("2026-11-25T01:00:00+00:00", "2026-11-25T01:00:00Z"),
            ("2026-01-01T05:30:00+09:30", "2025-12-31T20:00:00Z"),
            ("2024-02-28T23:00:00-02:00", "2024-02-29T01:00:00Z"),
            ("2026-12-31T23:59:59-00:01", "2027-01-01T00:00:59Z"),
        ] {
            assert_eq!(normalize_datetime(input).as_deref(), Some(want), "{input}");
        }
        for bad in [
            "2026-11-25",
            "2026-11-25T25:00:00Z",
            "2026-11-25T10:60:00Z",
            "2026-11-25T10:00:60Z",
            "2026-11-25T10:00:00",
            "2026-11-25T10:00:00.123Z",
            "2026-11-25T10:00:00.Z",
            "2026-11-25T10:00Z",
            "2026-11-25 10:00:00Z",
            "2026-11-25T10:00:00+0900",
            "2026-11-25T10:00:00+24:00",
            "2026-02-30T10:00:00Z",
            "0001-01-01T00:00:00+00:01",
            "9999-12-31T23:59:59-00:01",
            "明日の朝",
        ] {
            assert_eq!(normalize_datetime(bad), None, "{bad}");
        }
    }

    #[test]
    fn civil_round_trip() {
        for days in [-719_162, -1, 0, 1, 20_000, 2_932_896] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
    }
}
