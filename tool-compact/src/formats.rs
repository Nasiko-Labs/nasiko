//! `format: date-time` (RFC 3339 `date-time`) and `format: date` (RFC 3339 `full-date`) checks.
//! ASCII-only grammars, so byte access is safe; no slicing.

fn digits(b: &[u8], start: usize, n: usize) -> Option<u32> {
    let mut v: u32 = 0;
    for i in start..start + n {
        let d = *b.get(i)?;
        if !d.is_ascii_digit() {
            return None;
        }
        v = v * 10 + u32::from(d - b'0');
    }
    Some(v)
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400) => {
            29
        }
        2 => 28,
        _ => 0,
    }
}

/// `YYYY-MM-DD` prefix check; returns bytes consumed.
fn full_date(b: &[u8]) -> Option<usize> {
    let year = digits(b, 0, 4)?;
    if b.get(4) != Some(&b'-') {
        return None;
    }
    let month = digits(b, 5, 2)?;
    if b.get(7) != Some(&b'-') {
        return None;
    }
    let day = digits(b, 8, 2)?;
    (day >= 1 && day <= days_in_month(year, month)).then_some(10)
}

/// RFC 3339 `full-date`.
pub(crate) fn is_date(s: &str) -> bool {
    full_date(s.as_bytes()) == Some(s.len())
}

/// RFC 3339 `date-time`: `YYYY-MM-DDTHH:MM:SS[.frac](Z|±HH:MM)`.
pub(crate) fn is_date_time(s: &str) -> bool {
    let b = s.as_bytes();
    let Some(mut i) = full_date(b) else {
        return false;
    };
    if !matches!(b.get(i), Some(b'T' | b't')) {
        return false;
    }
    i += 1;
    let (Some(h), Some(m), Some(sec)) = (digits(b, i, 2), digits(b, i + 3, 2), digits(b, i + 6, 2))
    else {
        return false;
    };
    if b.get(i + 2) != Some(&b':') || b.get(i + 5) != Some(&b':') || h > 23 || m > 59 || sec > 60 {
        return false;
    }
    i += 8;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    match b.get(i) {
        Some(b'Z' | b'z') => i + 1 == b.len(),
        Some(b'+' | b'-') => {
            let (Some(oh), Some(om)) = (digits(b, i + 1, 2), digits(b, i + 4, 2)) else {
                return false;
            };
            b.get(i + 3) == Some(&b':') && oh <= 23 && om <= 59 && i + 6 == b.len()
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_time_accepts_rfc3339_and_rejects_near_misses() {
        assert!(is_date_time("2026-10-05T15:00:00+05:30"));
        assert!(is_date_time("2026-10-05T15:00:00.123Z"));
        assert!(is_date_time("2024-02-29T00:00:00z"));
        assert!(!is_date_time("2025-02-29T00:00:00Z"), "not a leap year");
        assert!(!is_date_time("2026-10-05 15:00:00Z"), "space separator");
        assert!(!is_date_time("2026-10-05T15:00:00"), "no offset");
        assert!(!is_date_time("2026-10-05T24:00:00Z"));
        assert!(!is_date_time("2026-10-05T15:00:00+0530"));
        assert!(!is_date_time("Monday 3pm"));
    }

    #[test]
    fn date_is_full_date_only() {
        assert!(is_date("2026-10-05"));
        assert!(!is_date("2026-13-05"));
        assert!(!is_date("2026-10-05T00:00:00Z"));
    }
}
