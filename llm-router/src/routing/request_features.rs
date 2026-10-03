//! Feature engine for the local request classifier — the **single source of truth** for how a
//! `(query, context)` pair becomes a feature vector.
//!
//! Both consumers call [`extract`]: the router's local backend
//! ([`super::local_classifier`]) at inference time, and the `dump_classifier_features`
//! example that writes the vectors the offline trainer fits on. Python never re-implements a
//! feature, so training and serving cannot drift apart (the failure mode the salience model's
//! separate Python feature mirror had to guard against with a parity script).
//!
//! The hashing primitives are the salience gate's ([`super::salience_classifier`]): FNV-1a
//! bucket + sign hashing over word and character n-grams. Grams are namespaced (`q:w:`,
//! `q:c:`, `q:p0:`, `c:w:`, `rx:`) so the same word in the query and in the context lands in
//! different buckets, and accumulation is in sorted bucket order so the vector — and every
//! score computed from it — is bitwise deterministic across processes.
//!
//! **Any change to normalization, gram construction, namespaces or dense features must bump
//! [`RC_FEATURE_ENGINE`] and retrain**: the weights file records the engine version it was
//! trained against and the loader refuses a mismatch.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

use super::classifier::{RequestType, regex_votes};
use super::salience_classifier::{char_ngrams, hash_to_bucket_n, word_ngrams, word_tokens};

/// Version tag of this feature engine, recorded in (and checked against) the weights file.
pub const RC_FEATURE_ENGINE: &str = "rc-v1";
/// Hashed feature space size.
pub const RC_NUM_BUCKETS: usize = 1 << 15;
/// Number of dense structural features (see the `D_*` index constants).
pub const RC_NUM_DENSE: usize = 16;
/// Query cap in chars (the head is kept: the ask is usually stated first).
pub const RC_QUERY_MAX_CHARS: usize = 4000;
/// Context cap in chars (the tail is kept: the most recent material is most relevant).
pub const RC_CONTEXT_MAX_CHARS: usize = 2000;

/// ln(1 + query word tokens).
pub const D_QUERY_TOKENS: usize = 0;
/// ln(1 + context word tokens).
pub const D_CONTEXT_TOKENS: usize = 1;
/// ln(1 + lines of query + context).
pub const D_LINES: usize = 2;
/// ln(1 + backtick characters in query + context).
pub const D_BACKTICKS: usize = 3;
/// Share of non-empty query + context lines that look like code.
pub const D_CODE_LINE_RATIO: usize = 4;
/// Error / stack-trace / HTTP-error marker present (0/1).
pub const D_ERROR_MARKER: usize = 5;
/// ln(1 + constraint markers and list items in query + context).
pub const D_CONSTRAINTS: usize = 6;
/// ln(1 + question marks in the query).
pub const D_QUESTION_MARKS: usize = 7;
/// Negation / scope-limiting wording in the query (0/1).
pub const D_NEGATION: usize = 8;
/// ln(1 + design vocabulary hits).
pub const D_DESIGN_VOCAB: usize = 9;
/// ln(1 + reasoning vocabulary hits).
pub const D_REASONING_VOCAB: usize = 10;
/// ln(1 + writing-deliverable vocabulary hits).
pub const D_WRITING_VOCAB: usize = 11;
/// Query repeated-token ratio: 1 − unique/total.
pub const D_REPEAT_RATIO: usize = 12;
/// Query non-alphanumeric ratio over non-whitespace characters.
pub const D_NON_ALNUM_RATIO: usize = 13;
/// Regex top-category vote share (0 when nothing fired).
pub const D_REGEX_TOP_SHARE: usize = 14;
/// Regex abstained — no pattern fired (0/1).
pub const D_REGEX_ABSTAINED: usize = 15;

/// A sparse hashed vector (strictly increasing buckets, no zero values) plus the dense
/// structural features.
#[derive(Debug, Clone, PartialEq)]
pub struct Features {
    pub hashed: Vec<(u32, f32)>,
    pub dense: [f32; RC_NUM_DENSE],
}

static SYSTEM_REMINDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<system-reminder>.*?</system-reminder>").unwrap());
static HORIZONTAL_WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]+").unwrap());
static ERROR_MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:error|exception|traceback|panic(?:ked)?|stack ?trace|errno|segfault)\b|\b[45]\d\d\b",
    )
    .unwrap()
});
static CONSTRAINT_MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:must|should not|without|do not|don't|ensure|keep|only|exactly|at least|no more than)\b")
        .unwrap()
});
static LIST_ITEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*(?:[-*•]|\d+[.)])\s").unwrap());
static NEGATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:do not|don't|never|no need|without|just|only)\b").unwrap());
static DESIGN_VOCAB: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:architect|design|schema|migration|rollout|scalab|trade-?off|component|idempot|state machine|api contract)")
        .unwrap()
});
static REASONING_VOCAB: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:why|diagnos|investigat|infer|prove|proof|reconstruct|root cause|probabilit|estimate|calculat|interleav)")
        .unwrap()
});
static WRITING_VOCAB: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:email|summar|rewrite|tone|bullet|audience|draft|blog|letter|announcement|customer|warmer|polite)")
        .unwrap()
});

/// Normalize text before featurizing: drop `<system-reminder>…</system-reminder>` blocks
/// (coding-agent boilerplate that says nothing about the ask), unify line endings, collapse
/// runs of spaces/tabs (newlines are kept — line structure is a feature), and trim.
pub fn normalize(text: &str) -> String {
    let stripped = SYSTEM_REMINDER.replace_all(text, " ");
    let unified = stripped.replace("\r\n", "\n");
    HORIZONTAL_WS.replace_all(&unified, " ").trim().to_string()
}

/// The first `max` chars of `s` (char-boundary safe).
fn head_chars(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// The last `max` chars of `s` (char-boundary safe).
fn tail_chars(s: &str, max: usize) -> &str {
    let n = s.chars().count();
    if n <= max {
        return s;
    }
    match s.char_indices().nth(n - max) {
        Some((i, _)) => &s[i..],
        None => s,
    }
}

fn ln1p_count(n: usize) -> f32 {
    (n as f32).ln_1p()
}

/// Whether a line looks like code: punctuation typical of code, or a code-ish opener.
fn is_code_like(line: &str) -> bool {
    let t = line.trim_start();
    line.contains([';', '{', '}', '(', ')', '='])
        || line.contains("->")
        || ["fn ", "def ", "class ", "//", "#include", "import "]
            .iter()
            .any(|p| t.starts_with(p))
}

/// Regex votes for `query` plus their total — public so the dump example writes exactly what
/// the router computes.
pub fn regex_vote_summary(query: &str) -> ([u8; 7], u32) {
    let votes = regex_votes(query);
    let total = votes.iter().map(|v| u32::from(*v)).sum();
    (votes, total)
}

/// Turn a `(query, context)` pair into features. Total: never panics, for any input.
pub fn extract(query: &str, context: Option<&str>) -> Features {
    let q_norm = normalize(query);
    let q = head_chars(&q_norm, RC_QUERY_MAX_CHARS);
    let c_norm = context.map(normalize).unwrap_or_default();
    let c = tail_chars(&c_norm, RC_CONTEXT_MAX_CHARS);

    // Regex votes run on the raw query, exactly as the router's regex classifier sees it.
    let (votes, vote_total) = regex_vote_summary(query);

    // --- hashed grams ---
    let q_tokens = word_tokens(q);
    let mut q_grams: Vec<String> = word_ngrams(&q_tokens, 2)
        .into_iter()
        .map(|g| format!("q:w:{g}"))
        .collect();
    q_grams.extend(char_ngrams(q, 3, 5).into_iter().map(|g| format!("q:c:{g}")));
    q_grams.extend(
        q_tokens
            .iter()
            .take(3)
            .enumerate()
            .map(|(i, t)| format!("q:p{i}:{t}")),
    );
    let c_tokens = word_tokens(c);
    let c_grams: Vec<String> = c_tokens.iter().map(|t| format!("c:w:{t}")).collect();

    let mut acc: BTreeMap<u32, f64> = BTreeMap::new();
    let mut add = |gram: &str, value: f64| {
        let (bucket, sign) = hash_to_bucket_n(gram, RC_NUM_BUCKETS);
        *acc.entry(bucket as u32).or_insert(0.0) += sign * value;
    };
    let q_norm_factor = 1.0 / (q_grams.len().max(1) as f64).sqrt();
    for g in &q_grams {
        add(g, q_norm_factor);
    }
    let c_norm_factor = 1.0 / (c_grams.len().max(1) as f64).sqrt();
    for g in &c_grams {
        add(g, c_norm_factor);
    }
    for rt in RequestType::ALL {
        let v = votes[rt.index()];
        if v > 0 {
            add(&format!("rx:{}", rt.as_str()), f64::from(v));
        }
    }
    let hashed: Vec<(u32, f32)> = acc
        .into_iter()
        .filter(|(_, v)| *v != 0.0)
        .map(|(b, v)| (b, v as f32))
        .collect();

    // --- dense features (lowercased, normalized text) ---
    let ql = q.to_lowercase();
    let cl = c.to_lowercase();
    let qc = if cl.is_empty() {
        ql.clone()
    } else {
        format!("{ql}\n{cl}")
    };
    let mut d = [0f32; RC_NUM_DENSE];
    d[D_QUERY_TOKENS] = ln1p_count(q_tokens.len());
    d[D_CONTEXT_TOKENS] = ln1p_count(c_tokens.len());
    d[D_LINES] = ln1p_count(qc.lines().count());
    d[D_BACKTICKS] = ln1p_count(qc.matches('`').count());
    let non_empty: Vec<&str> = qc.lines().filter(|l| !l.trim().is_empty()).collect();
    d[D_CODE_LINE_RATIO] = if non_empty.is_empty() {
        0.0
    } else {
        non_empty.iter().filter(|l| is_code_like(l)).count() as f32 / non_empty.len() as f32
    };
    d[D_ERROR_MARKER] = f32::from(u8::from(ERROR_MARKER.is_match(&qc)));
    d[D_CONSTRAINTS] =
        ln1p_count(CONSTRAINT_MARKER.find_iter(&qc).count() + LIST_ITEM.find_iter(&qc).count());
    d[D_QUESTION_MARKS] = ln1p_count(ql.matches('?').count());
    d[D_NEGATION] = f32::from(u8::from(NEGATION.is_match(&ql)));
    d[D_DESIGN_VOCAB] = ln1p_count(DESIGN_VOCAB.find_iter(&qc).count());
    d[D_REASONING_VOCAB] = ln1p_count(REASONING_VOCAB.find_iter(&qc).count());
    d[D_WRITING_VOCAB] = ln1p_count(WRITING_VOCAB.find_iter(&qc).count());
    d[D_REPEAT_RATIO] = if q_tokens.is_empty() {
        0.0
    } else {
        let unique: std::collections::BTreeSet<&String> = q_tokens.iter().collect();
        1.0 - unique.len() as f32 / q_tokens.len() as f32
    };
    let visible: Vec<char> = ql.chars().filter(|ch| !ch.is_whitespace()).collect();
    d[D_NON_ALNUM_RATIO] = if visible.is_empty() {
        0.0
    } else {
        visible.iter().filter(|ch| !ch.is_alphanumeric()).count() as f32 / visible.len() as f32
    };
    let top = votes.iter().copied().max().unwrap_or(0);
    d[D_REGEX_TOP_SHARE] = if vote_total == 0 {
        0.0
    } else {
        f32::from(top) / vote_total as f32
    };
    d[D_REGEX_ABSTAINED] = f32::from(u8::from(vote_total == 0));

    Features { hashed, dense: d }
}

/// The JSON form of a feature vector written by the dump example (`hashed` as `[bucket,
/// value]` pairs, `dense` as an array). Shared with the parity test so the dump can never
/// diverge from what inference computes.
pub fn features_to_json(f: &Features) -> serde_json::Value {
    serde_json::json!({
        "hashed": f.hashed.iter().map(|(b, v)| serde_json::json!([b, v])).collect::<Vec<_>>(),
        "dense": f.dense.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(f: &Features) -> (Vec<(u32, u32)>, Vec<u32>) {
        (
            f.hashed.iter().map(|(b, v)| (*b, v.to_bits())).collect(),
            f.dense.iter().map(|v| v.to_bits()).collect(),
        )
    }

    #[test]
    fn features_are_bitwise_deterministic_and_sorted() {
        for (q, c) in [
            (
                "Implement a parser for comma-separated integers",
                Some("Rust 2021"),
            ),
            ("hello", None),
            (
                "Explain why `next` returns the old value",
                Some("```rust\nfn next() {}\n```"),
            ),
        ] {
            let a = extract(q, c);
            let b = extract(q, c);
            assert_eq!(bits(&a), bits(&b));
            assert!(a.hashed.windows(2).all(|w| w[0].0 < w[1].0), "not sorted");
            assert!(
                a.hashed
                    .iter()
                    .all(|(b, v)| *v != 0.0 && (*b as usize) < RC_NUM_BUCKETS)
            );
        }
    }

    #[test]
    fn context_grams_are_namespaced_apart_from_query_grams() {
        for word in ["cache", "parser", "release", "migration", "invoice"] {
            let (q_bucket, _) = hash_to_bucket_n(&format!("q:w:{word}"), RC_NUM_BUCKETS);
            let (c_bucket, _) = hash_to_bucket_n(&format!("c:w:{word}"), RC_NUM_BUCKETS);
            assert_ne!(q_bucket, c_bucket, "{word}");
        }
        // The same text as query vs as context yields different vectors.
        assert_ne!(
            extract("cache", None).hashed,
            extract("x", Some("cache")).hashed
        );
    }

    #[test]
    fn system_reminder_blocks_are_stripped_before_featurizing() {
        let plain = extract("Fix the failing test", None);
        let noisy = extract(
            "Fix the failing test<system-reminder>\nTodo list: none. Be concise.\n</system-reminder>",
            None,
        );
        assert_eq!(bits(&plain), bits(&noisy));
        assert_eq!(normalize("a \t  b\r\nc  "), "a b\nc");
    }

    #[test]
    fn features_handle_adversarial_inputs() {
        let long = "word ".repeat(40_000);
        let cases: Vec<(String, Option<String>)> = vec![
            (String::new(), None),
            ("\u{1F600}\u{1F525}\u{1F44D}".into(), None),
            (long.clone(), Some(long)),
            ("nul\0byte".into(), Some("\0".into())),
            ("مرحبا كيف حالك".into(), None),
            ("混合 scripts ми́р 🙂 test".into(), Some("Ünïcödé".into())),
        ];
        for (q, c) in &cases {
            let f = extract(q, c.as_deref());
            assert!(
                f.dense
                    .iter()
                    .all(|v| v.is_finite() && *v >= 0.0 && *v < 20.0)
            );
            assert!(f.hashed.iter().all(|(_, v)| v.is_finite()));
        }
    }

    #[test]
    fn dump_matches_inference_features() {
        for (q, c) in [
            (
                "Fix typo in this Python comment",
                Some("No other files or changes needed."),
            ),
            ("What does Option::take() do in Rust?", None),
            (
                "Summarize the release notes in three bullets",
                Some("Release notes: ..."),
            ),
            (
                "Design a migration to queued processing",
                Some("POST /confirm returns 200"),
            ),
            ("", None),
        ] {
            let f = extract(q, c);
            let json = features_to_json(&f);
            let hashed: Vec<(u32, f32)> = json["hashed"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| (p[0].as_u64().unwrap() as u32, p[1].as_f64().unwrap() as f32))
                .collect();
            let dense: Vec<f32> = json["dense"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap() as f32)
                .collect();
            assert_eq!(hashed, f.hashed);
            assert_eq!(dense, f.dense.to_vec());
        }
    }

    #[test]
    fn dense_features_fire_on_their_cues() {
        let f = extract("Do not redesign anything: just rename it. Why?", None);
        assert_eq!(f.dense[D_NEGATION], 1.0);
        assert!(f.dense[D_QUESTION_MARKS] > 0.0);
        assert!(f.dense[D_REASONING_VOCAB] > 0.0);
        let g = extract("hello there", None);
        assert_eq!(g.dense[D_REGEX_ABSTAINED], 1.0);
        assert_eq!(g.dense[D_REGEX_TOP_SHARE], 0.0);
        let h = extract(
            "draft an email",
            Some("Traceback: error 500\nfn main() { x(); }"),
        );
        assert_eq!(h.dense[D_ERROR_MARKER], 1.0);
        assert!(h.dense[D_CODE_LINE_RATIO] > 0.0);
        assert!(h.dense[D_WRITING_VOCAB] > 0.0);
        assert_eq!(h.dense[D_REGEX_TOP_SHARE], 1.0);
    }
}
