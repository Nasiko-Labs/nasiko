//! The fixture corpus, as a gate rather than a demo.
//!
//! `examples/report.rs` prints the same corpus as a table for humans. This file asserts the two
//! properties that must hold before any of it is wired into a request path: nothing grows, and
//! nothing is classified as something it is not.

use nasiko_compress::{
    ContentType, Level, Policy, TypeMask, compress, contains_marker, detect, expand_tables,
};

struct Fixture {
    name: &'static str,
    body: &'static str,
    expect_type: ContentType,
    /// Whether the default policy (`json,log,diff,test`) should shrink it.
    expect_shrinks: bool,
}

fn corpus() -> Vec<Fixture> {
    vec![
        Fixture {
            name: "deployment_status.json",
            body: include_str!("fixtures/deployment_status.json"),
            expect_type: ContentType::Json,
            expect_shrinks: true,
        },
        Fixture {
            name: "container.log",
            body: include_str!("fixtures/container.log"),
            expect_type: ContentType::Log,
            expect_shrinks: true,
        },
        Fixture {
            name: "feature.diff",
            body: include_str!("fixtures/feature.diff"),
            expect_type: ContentType::Diff,
            expect_shrinks: true,
        },
        Fixture {
            name: "ripgrep_hits.txt",
            body: include_str!("fixtures/ripgrep_hits.txt"),
            expect_type: ContentType::SearchResults,
            // `search` is outside `TypeMask::DEFAULT` on purpose — see `TypeMask::DEFAULT`.
            expect_shrinks: false,
        },
        Fixture {
            name: "dashboard.html",
            body: include_str!("fixtures/dashboard.html"),
            expect_type: ContentType::Markup,
            expect_shrinks: false,
        },
        Fixture {
            name: "django_runtests_ok.txt",
            body: include_str!("fixtures/django_runtests_ok.txt"),
            expect_type: ContentType::TestOutput,
            expect_shrinks: true,
        },
        Fixture {
            name: "django_runtests_error.txt",
            body: include_str!("fixtures/django_runtests_error.txt"),
            expect_type: ContentType::TestOutput,
            expect_shrinks: true,
        },
        Fixture {
            name: "incident_summary.md",
            body: include_str!("fixtures/incident_summary.md"),
            expect_type: ContentType::Prose,
            expect_shrinks: false,
        },
    ]
}

fn default_on() -> Policy<'static> {
    Policy {
        enabled: true,
        ..Default::default()
    }
}

#[test]
fn every_fixture_detects_as_its_declared_type() {
    for f in corpus() {
        assert_eq!(detect(f.body), f.expect_type, "misdetected {}", f.name);
    }
}

/// The red-row gate. Caveman published an HTML case that grew 9.9%; we refuse to ship one.
#[test]
fn no_fixture_grows_under_any_level_or_mask() {
    for level in [Level::Conservative, Level::Balanced, Level::Aggressive] {
        let policy = Policy {
            enabled: true,
            types: TypeMask::ALL,
            level,
            ..Default::default()
        };
        for f in corpus() {
            let out = compress(f.body, &policy);
            assert!(
                out.compressed_bytes() <= out.original_bytes(),
                "{} grew at {level:?}: {} -> {}",
                f.name,
                out.original_bytes(),
                out.compressed_bytes()
            );
        }
    }
}

#[test]
fn default_policy_shrinks_exactly_the_fixtures_it_should() {
    for f in corpus() {
        let out = compress(f.body, &default_on());
        assert_eq!(
            out.is_changed(),
            f.expect_shrinks,
            "{} shrink expectation violated ({} -> {})",
            f.name,
            out.original_bytes(),
            out.compressed_bytes()
        );
    }
}

#[test]
fn the_failure_detail_in_each_payload_survives() {
    // The whole point: a payload is sent to a model because something in it went wrong.
    let cases = [
        (include_str!("fixtures/deployment_status.json"), "OOMKilled"),
        (include_str!("fixtures/container.log"), "connection reset"),
        (
            include_str!("fixtures/feature.diff"),
            "state.config.history_window",
        ),
    ];
    for (body, needle) in cases {
        let out = compress(body, &default_on());
        assert!(
            out.text().contains(needle),
            "compression dropped {needle:?}"
        );
    }
}

#[test]
fn the_search_fixture_keeps_its_diagnostic_hit_when_enabled() {
    let policy = Policy {
        enabled: true,
        types: TypeMask::DEFAULT.with(ContentType::SearchResults),
        ..Default::default()
    };
    let out = compress(include_str!("fixtures/ripgrep_hits.txt"), &policy);
    assert!(out.is_changed());
    assert!(out.text().contains("FIXME"), "dropped the diagnostic hit");
}

/// A shrunk output either says what it dropped, or dropped nothing: a JSON document rewritten
/// without a recovery handle (whitespace gone, lists tabled) must expand back to exactly the
/// original value.
#[test]
fn every_shrunk_fixture_says_so_in_the_text_or_lost_nothing() {
    for f in corpus() {
        let out = compress(f.body, &default_on());
        if !out.is_changed() {
            continue;
        }
        if let Ok(before) = serde_json::from_str::<serde_json::Value>(f.body) {
            assert_eq!(
                expand_tables(out.text()),
                Some(before),
                "{} lost data",
                f.name
            );
        } else {
            assert!(
                contains_marker(out.text()),
                "{} shrank silently — the model cannot tell data is missing",
                f.name
            );
        }
    }
}
