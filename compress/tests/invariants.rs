//! The seven invariants, exercised through the public API only.
//!
//! These are the tests that must still pass when a new compressor is added — they are the reason
//! the compressors are private and every rule lives in one funnel.

use nasiko_compress::{
    ContentType, Level, Policy, TypeMask, compress, contains_marker, detect, expand_tables,
};

/// Every type, the most aggressive level, and a recovery handle — the handle is what licenses
/// irreversible cuts (JSON item and string elision), so without it these invariants would only
/// ever see the lossless rewrite.
fn everything_on() -> Policy<'static> {
    Policy {
        enabled: true,
        min_bytes: 0,
        types: TypeMask::ALL,
        level: Level::Aggressive,
        recovery_ref: Some("h-invariants"),
        ..Default::default()
    }
}

/// One realistic sample per type, plus the adversarial shapes.
fn corpus() -> Vec<(&'static str, String)> {
    let mut log = String::new();
    for i in 0..300 {
        log.push_str(&format!("2026-01-01T00:00:00Z INFO handled request {i}\n"));
    }
    log.push_str("2026-01-01T00:05:00Z ERROR upstream timed out\n");
    log.push_str("    at src/main.rs:42\n");

    let json_items: Vec<String> = (0..200)
        .map(|i| format!(r#"{{"id":{i},"ok":true}}"#))
        .collect();
    let json = format!(
        r#"{{"count":200,"items":[{}],"error":null}}"#,
        json_items.join(",")
    );

    let mut diff =
        String::from("diff --git a/x.rs b/x.rs\n--- a/x.rs\n+++ b/x.rs\n@@ -1,80 +1,80 @@\n");
    for i in 0..40 {
        diff.push_str(&format!(" context {i}\n"));
    }
    diff.push_str("-old\n+new\n");
    for i in 40..80 {
        diff.push_str(&format!(" context {i}\n"));
    }

    let search: String = (0..200)
        .map(|i| format!("src/f{i}.rs:{i}:let v = {i};\n"))
        .collect();

    vec![
        ("log", log),
        ("json", json),
        ("diff", diff),
        ("search", search),
        ("prose", "The quick brown fox. ".repeat(500)),
        ("empty", String::new()),
        ("whitespace", "   \n\t\n  ".to_string()),
        (
            "malformed_json",
            r#"{"items":[{"id":1},{"id":2"#.to_string(),
        ),
        ("truncated_log", "2026-01-01T00:00:00Z INF".to_string()),
        ("multibyte", "café · naïve · 日本語 ".repeat(500)),
        ("single_line_huge", "x".repeat(200_000)),
        ("only_newlines", "\n".repeat(5000)),
    ]
}

#[test]
fn i1_malformed_input_of_every_shape_returns_byte_identical() {
    let malformed = [
        r#"{"items":[{"id":1},{"id":2"#,
        "@@ -1,2 +1,2 @@\n-a",
        "\u{0}\u{1}\u{2}",
        "{[}]",
    ];
    for input in malformed {
        let out = compress(input, &everything_on());
        assert_eq!(out.text(), input, "mutated malformed input: {input:?}");
    }
}

#[test]
fn i2_output_is_never_longer_than_input() {
    for (name, input) in corpus() {
        let out = compress(&input, &everything_on());
        assert!(
            out.compressed_bytes() <= out.original_bytes(),
            "{name} grew: {} -> {}",
            out.original_bytes(),
            out.compressed_bytes()
        );
    }
}

#[test]
fn i3_compression_is_idempotent() {
    for (name, input) in corpus() {
        let once = compress(&input, &everything_on()).into_text();
        let twice = compress(&once, &everything_on()).into_text();
        assert_eq!(once, twice, "{name} is not idempotent");
    }
}

#[test]
fn i3_a_third_pass_still_changes_nothing() {
    let (_, log) = corpus().into_iter().find(|(n, _)| *n == "log").unwrap();
    let a = compress(&log, &everything_on()).into_text();
    let b = compress(&a, &everything_on()).into_text();
    let c = compress(&b, &everything_on()).into_text();
    assert_eq!(a, b);
    assert_eq!(b, c);
}

#[test]
fn i4_multibyte_input_never_panics_and_stays_valid_utf8() {
    // Boundary-hostile by construction: a 3-byte char at every plausible truncation offset.
    for pad in [1, 2, 3, 299, 300, 301, 1999, 2000, 2001] {
        let input = format!("{}€{}", "a".repeat(pad), "b".repeat(pad));
        let out = compress(&input, &everything_on());
        assert!(std::str::from_utf8(out.text().as_bytes()).is_ok());
    }
}

#[test]
fn i4_multibyte_json_and_log_survive() {
    let json = format!(r#"{{"blob":"{}"}}"#, "日本語".repeat(3000));
    let log: String = (0..300)
        .map(|i| format!("2026-01-01T00:00:00Z INFO café · naïve · {i}\n"))
        .collect();
    for input in [json, log] {
        let out = compress(&input, &everything_on());
        assert!(!out.text().is_empty());
    }
}

#[test]
fn i5_compression_is_deterministic_across_repeated_calls() {
    for (name, input) in corpus() {
        let first = compress(&input, &everything_on()).into_text();
        for _ in 0..5 {
            assert_eq!(
                compress(&input, &everything_on()).into_text(),
                first,
                "{name} is not deterministic"
            );
        }
    }
}

#[test]
fn i6_input_above_the_ceiling_is_bounded_without_parsing() {
    let huge = format!(r#"{{"items":[{}]}}"#, "1,".repeat(2_000_000));
    let policy = Policy {
        max_input_bytes: 4096,
        ..everything_on()
    };
    let out = compress(&huge, &policy);
    assert!(
        out.compressed_bytes() < 16_384,
        "ceiling did not bound output"
    );
    assert!(contains_marker(out.text()));
}

#[test]
fn i7_every_elision_leaves_a_counted_marker() {
    for name in ["log", "json", "diff", "search"] {
        let (_, input) = corpus().into_iter().find(|(n, _)| *n == name).unwrap();
        let out = compress(&input, &everything_on());
        assert!(out.is_changed(), "{name} did not compress at all");
        assert!(
            contains_marker(out.text()),
            "{name} elided without a marker"
        );
        assert!(
            out.text().chars().any(|c| c.is_ascii_digit()),
            "{name} marker carries no count"
        );
    }
}

/// Without a recovery handle nothing JSON-shaped may be lost: whatever comes back must parse to
/// exactly the input value, at every level.
#[test]
fn json_without_a_recovery_handle_never_loses_data() {
    for level in [Level::Conservative, Level::Balanced, Level::Aggressive] {
        let policy = Policy {
            recovery_ref: None,
            level,
            ..everything_on()
        };
        for (name, input) in corpus() {
            if detect(&input) != ContentType::Json {
                continue;
            }
            let out = compress(&input, &policy);
            let before: serde_json::Value = serde_json::from_str(&input).unwrap();
            let after = expand_tables(out.text()).expect("output is still JSON");
            assert_eq!(
                before, after,
                "{name} lost data at {level:?} without a handle"
            );
        }
    }
}

#[test]
fn disabled_policy_is_a_no_op_for_the_whole_corpus() {
    for (name, input) in corpus() {
        let out = compress(&input, &Policy::default());
        assert_eq!(out.text(), input, "{name} changed under the default policy");
        assert!(!out.is_changed());
    }
}

#[test]
fn detection_is_stable_under_compression_for_structured_types() {
    // A compressed log must still look like a log, or a second seam misroutes it.
    let (_, log) = corpus().into_iter().find(|(n, _)| *n == "log").unwrap();
    let compressed = compress(&log, &everything_on()).into_text();
    assert_eq!(detect(&compressed), ContentType::Log);
}

#[test]
fn dry_run_never_mutates_but_always_reports() {
    let policy = Policy {
        dry_run: true,
        ..everything_on()
    };
    for name in ["log", "json", "diff", "search"] {
        let (_, input) = corpus().into_iter().find(|(n, _)| *n == name).unwrap();
        let out = compress(&input, &policy);
        assert_eq!(out.text(), input, "{name} was mutated in dry run");
        assert!(out.saved_bytes() > 0, "{name} reported no projected saving");
    }
}
