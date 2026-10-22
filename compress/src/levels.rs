//! Log levels: the word a logger writes to say how serious a line is.
//!
//! Shared by detection ([`crate::detect`] — is this text a log?) and the log compressor (which
//! lines must survive), so the two can never disagree about what a level is.
//!
//! A level counts in any letter case, but only where a logger would write one, so a level word
//! in a message (`instruction cache parity error corrected`) is not mistaken for one:
//!
//! * **Upper case, as a whole word** — `ERROR`, `WARN`, `FATAL` (log4j, Python, Go, BGL, …).
//! * **Any case, in brackets** — `[error]`, `[Warning]`, `<crit>`, `[INFO ]` (Apache, nginx,
//!   Serilog, padded log4j patterns).
//! * **Any case, after a level key** — `level=error`, `lvl=warn`, `"level":"fatal"`,
//!   `severity: critical` (logfmt, JSON loggers).
//! * **Any case, as the line's first word followed by a colon** — `fail: …`, `warn: …`
//!   (.NET console), `Error: …`.
//! * **klog / glog** — the level as one letter before the date: `E1022 12:00:00.000000 …`.

use crate::text;

/// Loggers write the level in a line's prefix, never deep in a payload.
const PREFIX_CHARS: usize = 120;

/// Every level word (upper case) and whether a line carrying it reports something going wrong —
/// such a line must survive compression.
const LEVELS: &[(&str, bool)] = &[
    // Something went wrong.
    ("EMERGENCY", true), // syslog
    ("EMERG", true),     // syslog, Apache, nginx
    ("ALERT", true),     // syslog, Apache, nginx
    ("CRITICAL", true),  // Python, .NET, syslog
    ("CRIT", true),      // syslog, Apache, nginx, .NET console (`crit:`)
    ("FATAL", true),     // log4j, logback, Go, BGL, Serilog
    ("FTL", true),       // Serilog
    ("PANIC", true),     // Go, Rust, Linux kernel
    ("SEVERE", true),    // java.util.logging, BGL
    ("FAILURE", true),   // BGL
    ("FAIL", true),      // .NET console (`fail:`), test runners
    ("ERROR", true),     // almost every logger
    ("ERR", true),       // syslog, Serilog
    ("WARNING", true),   // Python, java.util.logging, .NET, syslog, BGL
    ("WARN", true),      // log4j, logback, Go, Apache, nginx, .NET console
    ("WRN", true),       // Serilog
    // Routine.
    ("NOTICE", false),      // syslog, Apache, nginx
    ("INFORMATION", false), // .NET
    ("INFO", false),        // almost every logger
    ("INF", false),         // Serilog
    ("CONFIG", false),      // java.util.logging
    ("DEBUG", false),       // almost every logger
    ("DBUG", false),        // .NET console (`dbug:`)
    ("DBG", false),         // Serilog
    ("TRACE", false),       // log4j, logback, .NET, Apache (`trace1`…`trace8`)
    ("TRCE", false),        // .NET console (`trce:`)
    ("VERBOSE", false),     // .NET
    ("VRB", false),         // Serilog
    ("FINE", false),        // java.util.logging
    ("FINER", false),       // java.util.logging
    ("FINEST", false),      // java.util.logging
];

/// Keys that name a level in structured logs (`level=error`, `"severity":"warn"`).
const LEVEL_KEYS: &[&[u8]] = &[b"level", b"lvl", b"loglevel", b"log.level", b"severity"];

/// The levels a line carries: `Some(true)` when one of them reports a failure, `Some(false)` when
/// it only has routine ones, `None` when it has none.
pub(crate) fn severity(line: &str) -> Option<bool> {
    if let Some(severe) = klog_level(line) {
        return Some(severe);
    }
    let prefix = text::take_chars(line, PREFIX_CHARS).as_bytes();
    let mut found = None;
    let mut i = 0;
    while i < prefix.len() {
        if !is_word_byte(prefix[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < prefix.len() && is_word_byte(prefix[i]) {
            i += 1;
        }
        if let Some(severe) = level_at(prefix, start, i) {
            if severe {
                return Some(true);
            }
            found = Some(false);
        }
    }
    found
}

pub(crate) fn has_level(line: &str) -> bool {
    severity(line).is_some()
}

pub(crate) fn is_severe(line: &str) -> bool {
    severity(line) == Some(true)
}

/// Letters, digits and `_` make one word, so `ERR_CONNECTION` or `TERROR` is never a level.
fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The level written as `bytes[start..end]`, if that word is one and stands where a level goes.
fn level_at(bytes: &[u8], start: usize, end: usize) -> Option<bool> {
    let word = bytes.get(start..end)?;
    // Apache's `trace1` … `trace8`: a level followed only by digits.
    let letters_end = word
        .iter()
        .rposition(|b| !b.is_ascii_digit())
        .map_or(0, |p| p + 1);
    let letters = word.get(..letters_end)?;
    if letters.is_empty() || !letters.iter().all(u8::is_ascii_alphabetic) {
        return None;
    }
    let severe = LEVELS
        .iter()
        .find(|(level, _)| level.as_bytes().eq_ignore_ascii_case(letters))
        .map(|(_, severe)| *severe)?;
    let upper = letters.iter().all(u8::is_ascii_uppercase) && word.len() == letters.len();
    (upper
        || bracketed(bytes, start, end)
        || after_level_key(bytes, start)
        || leads_with_colon(bytes, start, end))
    .then_some(severe)
}

/// `[error]`, `<crit>`, `(warn)`, `[INFO ]`.
fn bracketed(bytes: &[u8], start: usize, end: usize) -> bool {
    let before = start.checked_sub(1).and_then(|i| bytes.get(i));
    let after = bytes
        .get(end..)
        .and_then(|rest| rest.iter().find(|b| **b != b' '));
    matches!(
        (before, after),
        (Some(b'['), Some(b']')) | (Some(b'<'), Some(b'>')) | (Some(b'('), Some(b')'))
    )
}

/// `level=error`, `lvl=warn`, `"level":"fatal"`, `severity: critical`.
fn after_level_key(bytes: &[u8], start: usize) -> bool {
    let mut i = start;
    let skip = |i: &mut usize, what: &[u8]| {
        while *i > 0 && bytes.get(*i - 1).is_some_and(|b| what.contains(b)) {
            *i -= 1;
        }
    };
    skip(&mut i, b"\"' ");
    if i == 0 || !matches!(bytes.get(i - 1), Some(b'=' | b':')) {
        return false;
    }
    i -= 1;
    skip(&mut i, b"\"' ");
    let Some(head) = bytes.get(..i) else {
        return false;
    };
    LEVEL_KEYS.iter().any(|key| {
        head.len() >= key.len()
            && head
                .get(head.len() - key.len()..)
                .is_some_and(|tail| tail.eq_ignore_ascii_case(key))
            && head
                .len()
                .checked_sub(key.len() + 1)
                .and_then(|j| head.get(j))
                .is_none_or(|b| !is_word_byte(*b))
    })
}

/// The line's first word, followed by a colon: `fail: …`, `Error: …`.
fn leads_with_colon(bytes: &[u8], start: usize, end: usize) -> bool {
    bytes
        .get(..start)
        .is_some_and(|before| before.iter().all(u8::is_ascii_whitespace))
        && bytes.get(end) == Some(&b':')
}

/// klog / glog: `E1022 12:00:00.000000 …` — level letter, month and day, then the time.
fn klog_level(line: &str) -> Option<bool> {
    let b = line.trim_start().as_bytes();
    let shape = b.len() >= 9
        && b.get(1..5)
            .is_some_and(|d| d.iter().all(u8::is_ascii_digit))
        && b.get(5) == Some(&b' ')
        && b.get(6..8)
            .is_some_and(|d| d.iter().all(u8::is_ascii_digit))
        && b.get(8) == Some(&b':');
    if !shape {
        return None;
    }
    match b.first() {
        Some(b'I') => Some(false),
        Some(b'W' | b'E' | b'F') => Some(true),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upper_case_levels_count_anywhere_in_the_prefix() {
        assert_eq!(severity("2026-01-01T00:00:00Z ERROR boom"), Some(true));
        assert_eq!(
            severity("2015-07-29 19:17:24,477 - WARN  [SendWorker] - x"),
            Some(true)
        );
        assert_eq!(
            severity(
                "- 1118193358 2005.06.07 R11-M0 2005-06-07-18.15.58 R11-M0 RAS APP FATAL ciod: x"
            ),
            Some(true)
        );
        assert_eq!(
            severity("081109 203615 148 INFO dfs.DataNode$PacketResponder: x"),
            Some(false)
        );
        for word in [
            "EMERG", "ALERT", "CRITICAL", "PANIC", "SEVERE", "FAILURE", "FAIL", "ERR", "WRN", "FTL",
        ] {
            assert_eq!(severity(&format!("x {word} y")), Some(true), "{word}");
        }
    }

    #[test]
    fn lower_case_levels_count_where_a_logger_writes_them() {
        // Apache and nginx
        assert_eq!(
            severity("[Sun Dec 04 05:15:09 2005] [error] [client 1.2.3.4] Directory index"),
            Some(true)
        );
        assert_eq!(
            severity("2015/01/01 12:00:00 [crit] 1234#0: *1 connect() failed"),
            Some(true)
        );
        assert_eq!(
            severity("[Sun Dec 04 04:47:44 2005] [notice] workerEnv.init() ok"),
            Some(false)
        );
        assert_eq!(
            severity("[Sun Dec 04 04:47:44 2005] [trace3] details"),
            Some(false)
        );
        // logfmt and JSON
        assert_eq!(
            severity("ts=2026-01-01 level=error msg=\"db down\""),
            Some(true)
        );
        assert_eq!(
            severity("{\"time\":\"12:00\",\"level\":\"fatal\",\"msg\":\"x\"}"),
            Some(true)
        );
        assert_eq!(
            severity("{\"severity\": \"warning\", \"message\": \"x\"}"),
            Some(true)
        );
        assert_eq!(severity("ts=2026-01-01 lvl=info msg=ok"), Some(false));
        // .NET console, Serilog, padded log4j
        assert_eq!(severity("fail: Microsoft.AspNetCore.Server[1]"), Some(true));
        assert_eq!(severity("[12:00:01 ERR] Unhandled exception"), Some(true));
        assert_eq!(severity("[INFO ] 2026-01-01 started"), Some(false));
        assert_eq!(severity("Error: connection refused"), Some(true));
    }

    #[test]
    fn level_words_inside_a_message_do_not_count() {
        // BGL's own INFO line about a corrected error must not read as an error.
        assert_eq!(
            severity(
                "- 1117838570 2005.06.03 R02-M1 RAS KERNEL INFO instruction cache parity error corrected"
            ),
            Some(false)
        );
        assert_eq!(
            severity("Jul 10 04:04:33 combo cups: cupsd shutdown succeeded"),
            None
        );
        assert_eq!(
            severity("the error budget is fine and nothing failed"),
            None
        );
        assert_eq!(severity("ERRORS=0 TERROR ERR_CONNECTION_RESET"), None);
        assert_eq!(severity("Error handling improved in this release"), None);
        assert_eq!(severity("mylevel=error"), None);
    }

    #[test]
    fn klog_letter_prefixes_count() {
        assert_eq!(
            severity("E1022 12:00:00.000000    1 main.go:42] boom"),
            Some(true)
        );
        assert_eq!(
            severity("W1022 12:00:00.000000    1 main.go:42] slow"),
            Some(true)
        );
        assert_eq!(
            severity("I1022 12:00:00.000000    1 main.go:42] ok"),
            Some(false)
        );
        assert_eq!(severity("E1 is a road"), None);
    }

    #[test]
    fn multibyte_prefixes_do_not_panic() {
        assert_eq!(severity("naïve · café · [error] ✓"), Some(true));
        assert_eq!(severity(&"é".repeat(500)), None);
    }
}
