//! The single funnel. Every invariant lives here rather than in each compressor, so a compressor
//! added later inherits all of them and cannot regress any — the only way to bypass the
//! never-grows check would be making a compressor `pub`, which is a visible API change.

use crate::compressed::Compressed;
use crate::compressors;
use crate::content_type::ContentType;
use crate::detect::detect;
use crate::marker;
use crate::policy::Policy;
use crate::text;

/// Characters kept either side of the elision when an input exceeds `max_input_bytes`.
const CEILING_HEAD_CHARS: usize = 2000;
const CEILING_TAIL_CHARS: usize = 1000;

/// Detect `input`'s shape and compress it.
///
/// Infallible: anything that cannot be compressed — malformed input, a type with no compressor,
/// a policy that excludes it, an output that would not be smaller — comes back unchanged.
pub fn compress<'a>(input: &'a str, policy: &Policy<'_>) -> Compressed<'a> {
    // Bounded time, checked before detection: the Json rule needs a full parse, and a huge
    // malformed blob must not pay for one before any ceiling applies.
    if input.len() > policy.max_input_bytes {
        return match text::head_tail(
            input,
            CEILING_HEAD_CHARS,
            CEILING_TAIL_CHARS,
            policy.recovery_ref,
        ) {
            Some(out) if out.len() < input.len() => {
                Compressed::shrunk(input, ContentType::Prose, out)
            }
            _ => Compressed::unchanged(input, ContentType::Prose),
        };
    }

    let kind = detect(input);

    // Idempotency: input already carrying one of our breadcrumbs is our own prior output (or
    // contains it), and re-eliding around a marker would produce different bytes on every pass.
    //
    // The size floor exists because a marker costs about what eliding a small payload saves.
    // JSON is the exception: below the floor it is still rewritten, but only losslessly (no
    // recovery handle, so nothing is cut) — dropping whitespace from a 600-byte tool schema
    // costs no marker and loses nothing.
    let below_floor = input.len() < policy.min_bytes;
    if !policy.enabled
        || (below_floor && kind != ContentType::Json)
        || !policy.types.contains(kind)
        || marker::contains_marker(input)
    {
        return Compressed::unchanged(input, kind);
    }
    let lossless_only;
    let policy = if below_floor {
        lossless_only = Policy {
            recovery_ref: None,
            ..*policy
        };
        &lossless_only
    } else {
        policy
    };

    // Fail-closed: compressors return `None` for "no saving found", which is indistinguishable
    // from "could not parse this". There is no error path out of here.
    match compressors::dispatch(kind, input, policy) {
        // Never grows — the one length check, which no compressor can skip.
        Some(out) if !out.is_empty() && out.len() < input.len() => {
            if policy.dry_run {
                Compressed::projected(input, kind, out.len())
            } else {
                Compressed::shrunk(input, kind, out)
            }
        }
        _ => Compressed::unchanged(input, kind),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_type::TypeMask;
    use crate::marker::contains_marker;

    fn enabled() -> Policy<'static> {
        Policy {
            enabled: true,
            min_bytes: 0,
            types: TypeMask::ALL,
            ..Default::default()
        }
    }

    #[test]
    fn small_json_below_the_floor_is_rewritten_losslessly() {
        // A BFCL-style tool schema: 600 bytes, mostly indentation.
        let schema = serde_json::json!([{
            "name": "calculate_triangle_area",
            "description": "Calculate the area of a triangle given its base and height.",
            "parameters": {"type": "dict", "properties": {
                "base": {"type": "integer", "description": "The base of the triangle."},
                "height": {"type": "integer", "description": "The height of the triangle."}
            }, "required": ["base", "height"]}
        }]);
        let input = serde_json::to_string_pretty(&schema).unwrap();
        let policy = Policy {
            min_bytes: 2048,
            recovery_ref: Some("h-ignored-below-the-floor"),
            ..enabled()
        };
        assert!(input.len() < policy.min_bytes);

        let out = compress(&input, &policy);

        assert!(out.is_changed(), "indentation should go");
        let back: serde_json::Value = serde_json::from_str(out.text()).unwrap();
        assert_eq!(back, schema, "nothing may be cut below the floor");
        assert!(!contains_marker(out.text()));
    }

    #[test]
    fn small_non_json_below_the_floor_is_untouched() {
        let log = "2026-01-01T00:00:00Z INFO a\n".repeat(20);
        let policy = Policy {
            min_bytes: 2048,
            ..enabled()
        };
        assert!(!compress(&log, &policy).is_changed());
    }

    fn noisy_log() -> String {
        let mut s = String::new();
        for i in 0..200 {
            s.push_str(&format!(
                "2026-01-01T00:00:{:02}Z INFO handled request {i}\n",
                i % 60
            ));
        }
        s.push_str("2026-01-01T00:05:00Z ERROR upstream timed out\n");
        s
    }

    #[test]
    fn disabled_policy_returns_input_unchanged() {
        let input = noisy_log();
        let out = compress(&input, &Policy::default());
        assert!(!out.is_changed());
        assert_eq!(out.text(), input);
    }

    #[test]
    fn input_below_min_bytes_is_untouched() {
        let policy = Policy {
            min_bytes: 1 << 20,
            ..enabled()
        };
        let input = noisy_log();
        assert!(!compress(&input, &policy).is_changed());
    }

    #[test]
    fn type_excluded_by_mask_is_untouched() {
        let policy = Policy {
            types: TypeMask::NONE,
            ..enabled()
        };
        let input = noisy_log();
        assert!(!compress(&input, &policy).is_changed());
    }

    #[test]
    fn dry_run_reports_the_saving_without_changing_the_text() {
        let input = noisy_log();
        let policy = Policy {
            dry_run: true,
            ..enabled()
        };
        let out = compress(&input, &policy);
        assert_eq!(out.text(), input);
        assert!(!out.is_changed());
        assert!(
            out.saved_bytes() > 0,
            "dry run should still report a saving"
        );
    }

    #[test]
    fn output_carrying_a_marker_is_not_compressed_again() {
        let input = noisy_log();
        let policy = enabled();
        let once = compress(&input, &policy).into_text();
        let twice = compress(&once, &policy);
        assert_eq!(twice.text(), once);
    }

    #[test]
    fn oversized_input_is_truncated_without_parsing() {
        let input = "a".repeat(10_000);
        let policy = Policy {
            max_input_bytes: 1000,
            ..enabled()
        };
        let out = compress(&input, &policy);
        assert!(out.is_changed());
        assert!(out.text().len() < input.len());
        assert!(marker::contains_marker(out.text()));
    }

    #[test]
    fn oversized_but_incompressible_input_survives_intact() {
        // Shorter than head+tail, so `head_tail` declines and the ceiling must not mangle it.
        let input = "x".repeat(50);
        let policy = Policy {
            max_input_bytes: 10,
            ..enabled()
        };
        assert_eq!(compress(&input, &policy).text(), input);
    }

    #[test]
    fn type_with_no_compressor_is_untouched() {
        let prose = "word ".repeat(2000);
        let out = compress(&prose, &enabled());
        assert_eq!(out.content_type(), ContentType::Prose);
        assert!(!out.is_changed());
    }
}
