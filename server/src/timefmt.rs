//! 時刻の文字列の形の検査。時刻の解釈と比較はPostgreSQLで行うが、
//! PostgreSQLは `tomorrow` や `infinity` のような特別な値も受け付けるので、先に RFC 3339 の形に限る。

/// `YYYY-MM-DDTHH:MM:SS(.fraction)?(Z|±HH:MM)` の形か。
pub fn is_rfc3339(s: &str) -> bool {
    let b = s.as_bytes();
    let digits =
        |r: std::ops::Range<usize>| b.get(r).is_some_and(|x| x.iter().all(u8::is_ascii_digit));
    if b.len() < 20
        || !digits(0..4)
        || b[4] != b'-'
        || !digits(5..7)
        || b[7] != b'-'
        || !digits(8..10)
        || !matches!(b[10], b'T' | b't')
        || !digits(11..13)
        || b[13] != b':'
        || !digits(14..16)
        || b[16] != b':'
        || !digits(17..19)
    {
        return false;
    }
    let mut rest = &b[19..];
    if rest.first() == Some(&b'.') {
        let n = rest[1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if n == 0 {
            return false;
        }
        rest = &rest[1 + n..];
    }
    match rest {
        [b'Z' | b'z'] => true,
        [b'+' | b'-', h1, h2, b':', m1, m2] => [h1, h2, m1, m2].iter().all(|c| c.is_ascii_digit()),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        for ok in [
            "2026-10-13T00:00:00Z",
            "2026-10-13T00:00:00.123Z",
            "2026-10-13T09:00:00+09:00",
            "2026-10-13t00:00:00z",
        ] {
            assert!(is_rfc3339(ok), "{ok}");
        }
        for bad in [
            "tomorrow",
            "infinity",
            "now",
            "2026-10-13",
            "2026-10-13 00:00:00Z",
            "2026-10-13T00:00:00",
            "2026-10-13T00:00:00.Z",
            "2026-10-13T00:00:00+0900",
            "",
        ] {
            assert!(!is_rfc3339(bad), "{bad}");
        }
    }
}
