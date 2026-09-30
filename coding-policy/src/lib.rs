//! Shared "minimal-code" decision-ladder policy for coding-style agents
//! (docs/CODING_AGENT_MINIMALISM.md).
//!
//! The ladder text ([`minimal_code_addendum`]) is injected by the control
//! plane directly into the outgoing task message at A2A dispatch time
//! (`oss/server/src/router/a2a_dispatch.rs`), not built into any agent's own
//! system prompt — that's what lets it apply to any coding-type agent
//! (skills containing "code"), including a third party's, without that agent
//! needing to know this crate exists. A dependent agent only needs the two
//! functions below: read [`minimal_code_enabled`]/[`self_review_enabled`]
//! once at boot, and before returning a final answer, check
//! [`wants_self_review`] — if true, push [`SELF_REVIEW_PROMPT`] as one more
//! turn and use that response instead. Self-review stays agent-side (unlike
//! the ladder) because it needs the agent's own visibility into whether it
//! actually wrote or edited a file, which the control plane can't see from
//! the outside.
//!
//! [`minimal_code_addendum`] picks between three fixed texts by prior turn
//! count alone (a session-shape fact the control plane already has for
//! free) — it does not try to classify what a request is asking for from
//! its wording. That classification is left to the model itself: the ladder
//! text states the actual criterion ("search only when something equivalent
//! plausibly already exists") and trusts the model's own reading of the
//! request to apply it, since a CP-side heuristic trying to pattern-match
//! every phrasing of "this needs a search" would be strictly worse at
//! exactly the thing the model is already good at.
//!
//! This is the canonical source. A dependent agent keeps its own committed
//! copy under `vendor/coding-policy/` (`docker build` only ever sees that
//! agent's own directory, so a `../` path dependency can't resolve inside the
//! container) — re-run `sync-vendor.sh <agent-dir>` after editing this file to
//! update every vendored copy.

/// Read `CODING_AGENT_MINIMAL_CODE` from the environment. Off by default so the
/// ladder's effect can be A/B'd per deployment rather than assumed.
pub fn minimal_code_enabled() -> bool {
    std::env::var("CODING_AGENT_MINIMAL_CODE")
        .map(|v| v == "true")
        .unwrap_or(false)
}

/// Read `CODING_AGENT_SELF_REVIEW` from the environment. On by default —
/// meant as a temporary testing knob (isolate the ladder's effect on
/// generation itself from Phase 2's extra review turn), not a permanent
/// removal of Phase 2. Independent of [`minimal_code_enabled`]: Phase 2 only
/// ever fires when the ladder is also on (see [`wants_self_review`]), so this
/// only has any effect on a `minimal_code=true` deployment.
pub fn self_review_enabled() -> bool {
    std::env::var("CODING_AGENT_SELF_REVIEW")
        .map(|v| v != "false")
        .unwrap_or(true)
}

/// Whole words that mark a piece of an agent card as code work.
///
/// Matched as word *prefixes* against whole words, never as substrings. That distinction is
/// the entire fix: the previous `ILIKE '%code%'` (and the settings page's mirrored
/// `/code/i`) matched `encode`, `decode` and `barcode` while **missing `coding`**, which
/// contains no "code" at all — c-o-d-i-n-g. Any uploaded agent whose card said "coding
/// assistant" was therefore classified as not-a-coding-agent, so the settings toggle never
/// rendered and the ladder never injected, with no error anywhere.
///
/// Deliberately absent: `develop` and `engineer`. They would match "business development"
/// and "prompt engineering", and an agent that does software work almost always also says
/// "software", which is matched.
const CODING_TERMS: [&str; 8] = [
    "cod",      // code, codes, coding, coder, codebase, codegen
    "program",  // program, programming, programmer
    "software", // software engineering, software development
    "refactor", "debug", "bug", // bug fixing, bugfix
    "lint", "compil", // compile, compiler, compilation
];

/// Whether one piece of an agent card — a skill id, name or tag — reads as code work.
///
/// Recall is favoured over precision on purpose, because the two errors are not symmetric:
/// a false negative silently withholds a feature an operator explicitly switched on, while a
/// false positive only offers a toggle that is off by default. Callers test each field they
/// have; this function deliberately knows nothing about the shape of an agent card, which is
/// what keeps this crate dependency-free and lets the dispatch path, the catalog API and the
/// settings page share one answer instead of three implementations that drift.
pub fn mentions_coding(text: &str) -> bool {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .any(|word| {
            CODING_TERMS.iter().any(|term| {
                word.get(..term.len())
                    .is_some_and(|head| head.eq_ignore_ascii_case(term))
            })
        })
}

/// Continuing, early: this session has a few prior turns. Framed as a
/// judgment call, not a mandatory first step — a CP-side heuristic trying to
/// pre-classify every possible phrasing of "this needs a search" vs. "this
/// doesn't" would be exactly the kind of brittle keyword-matching that breaks
/// on the first request phrased differently than whatever it was tuned
/// against. The model reading the actual request already does that
/// classification correctly, for any phrasing, as part of understanding the
/// request at all — so state the criterion and let it apply that judgment,
/// instead of a second, cruder classifier trying to out-guess it from
/// outside. A rename, a removal, or a fix to something the request already
/// names doesn't need a search; a request that plausibly overlaps with
/// existing functionality does.
const MINIMAL_CODE_ADDENDUM_CONTINUING: &str = "\n\
- Only search this workspace first (using whatever file-reading or search tools you have) when \
it's plausible something equivalent already exists that you haven't already seen this session. A \
rename, a removal, or a fix to something the request already names — or something you've \
already located earlier in this session — doesn't need a fresh search; use your judgment on \
which this is, rather than treating search as a mandatory first step for every request.
- Once you know there's nothing to reuse, prefer the language's standard library or an \
already-installed dependency over writing something from scratch.
- If the request is for example or reference code (\"give me code for X\", \"write a function \
that does Y\") rather than an explicit ask to add or change something in this workspace, just \
write the code directly in your response. Do not create a file, set up a project scaffold, or \
run a build or test cycle for a standalone example — the person asking has no access to your \
sandbox and wants \
something to read or copy, not a file left behind where they can't reach it.
- This does not apply to trust-boundary checks, error handling for real failure modes, \
security, or data-loss prevention — those are never skipped for brevity.";

/// Continuing, established: several turns in, the model has already built up
/// real context of this workspace from its own earlier tool calls this
/// session. Same judgment-call framing as the early-continuing variant, but
/// stated more assertively toward *not* re-searching, since by this point in
/// a session redundant search is the more likely failure mode than missing
/// something genuinely new.
const MINIMAL_CODE_ADDENDUM_ESTABLISHED: &str = "\n\
- You've already explored this workspace across earlier turns in this session — don't re-search \
out of habit. Only search again (using whatever file-reading or search tools you have) if this \
specific request plausibly touches code you haven't already seen; a rename, a removal, or a fix to \
something already named or already located needs no search at all.
- Prefer the language's standard library or an already-installed dependency over writing \
something from scratch when one obviously already covers the need.
- If the request is for example or reference code (\"give me code for X\", \"write a function \
that does Y\") rather than an explicit ask to add or change something in this workspace, just \
write the code directly in your response. Do not create a file, set up a project scaffold, or \
run a build or test cycle for a standalone example — the person asking has no access to your \
sandbox and wants \
something to read or copy, not a file left behind where they can't reach it.
- This does not apply to trust-boundary checks, error handling for real failure modes, \
security, or data-loss prevention — those are never skipped for brevity.";

/// Below this many prior turns, "continuing" stays in the early-session
/// wording; at or above it, the established wording takes over. Picked as a
/// coarse midpoint for an 18-turn benchmark session, not a tuned constant —
/// the two variants differ only in emphasis, so this threshold being off by
/// a turn or two either way costs nothing.
const ESTABLISHED_SESSION_THRESHOLD: usize = 4;

/// Fresh-start ladder: this is the first message in the session, so there is
/// nothing in the workspace yet to search for — skips the search-first step
/// entirely, since on a genuinely empty workspace it only spends tokens
/// finding nothing, with zero payoff. Confirmed empirically (chat 2026-09-22):
/// on a from-scratch task, minimal-code mode cost noticeably more tokens per
/// turn than not having it on at all, driven by exactly this kind of
/// search-with-nothing-to-find overhead. Still keeps the stdlib/dependency
/// nudge, since that costs nothing extra — it doesn't require searching
/// anything, just recalling what's already installed.
const MINIMAL_CODE_ADDENDUM_FRESH_START: &str = "\n\
- This is the first request in this session — there is nothing in the workspace yet to search \
for or reuse, so don't spend a turn searching an empty workspace before writing. Do still prefer \
the language's standard library or an already-installed dependency over writing something from \
scratch when one obviously already covers the need.
- If the request is for example or reference code (\"give me code for X\", \"write a function \
that does Y\") rather than an explicit ask to add or change something in this workspace, just \
write the code directly in your response. Do not create a file, set up a project scaffold, or \
run a build or test cycle for a standalone example — the person asking has no access to your \
sandbox and wants \
something to read or copy, not a file left behind where they can't reach it.
- This does not apply to trust-boundary checks, error handling for real failure modes, \
security, or data-loss prevention — those are never skipped for brevity.";

/// Which ladder text to inject, given how many turns this session already
/// had before this one. Callers pass `history.messages.iter().filter(|m|
/// m.role == "user").count()` — the control plane already has this for free
/// (`SessionHistory`, fetched once per dispatch for the conversation-context
/// merge), no extra query or LLM call needed. Three tiers, not a classifier:
/// this is a session-shape signal (a count CP already has), not an attempt to
/// guess per-request intent from its wording — see the doc comment on
/// [`MINIMAL_CODE_ADDENDUM_CONTINUING`] for why that distinction matters.
pub fn minimal_code_addendum(prior_turn_count: usize) -> &'static str {
    if prior_turn_count == 0 {
        MINIMAL_CODE_ADDENDUM_FRESH_START
    } else if prior_turn_count < ESTABLISHED_SESSION_THRESHOLD {
        MINIMAL_CODE_ADDENDUM_CONTINUING
    } else {
        MINIMAL_CODE_ADDENDUM_ESTABLISHED
    }
}

/// One forced self-review turn before the final answer, only when the ladder is
/// on and the session actually wrote or edited a file — see [`wants_self_review`].
pub const SELF_REVIEW_PROMPT: &str = "\
Before finishing: look back at the edits you made. Is there anything you wrote that duplicates \
existing code, reimplements a stdlib/dependency feature, or wasn't needed to satisfy the original \
request? If so, say what you'd remove and why, then stop — do not make further edits unless asked. \
Otherwise, confirm your changes are minimal and give your summary.";

/// Whether the self-review turn ([`SELF_REVIEW_PROMPT`]) should run: only when
/// the ladder is on, the session wrote/edited a file, and there's a real answer
/// to review. Triggered, not universal — a read-only session, or the ladder
/// being off, never sees this extra turn.
pub fn wants_self_review(minimal_code: bool, wrote_code: bool, final_text: &str) -> bool {
    minimal_code && wrote_code && !final_text.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_start_skips_search_first_but_keeps_stdlib_nudge() {
        let addendum = minimal_code_addendum(0);
        assert!(addendum.contains("nothing in the workspace yet to search for"));
        assert!(addendum.contains("standard library"));
        assert!(!addendum.contains("search this workspace first"));
    }

    #[test]
    fn early_continuing_frames_search_as_a_judgment_call() {
        let addendum = minimal_code_addendum(1);
        assert!(addendum.contains("search this workspace first"));
        assert!(addendum.contains("use your judgment"));
        assert!(!addendum.contains("nothing in the workspace yet"));
    }

    #[test]
    fn established_session_leans_against_resurching() {
        let addendum = minimal_code_addendum(ESTABLISHED_SESSION_THRESHOLD);
        assert!(addendum.contains("already explored this workspace"));
        assert!(addendum.contains("don't re-search out of habit"));
    }

    #[test]
    fn threshold_is_the_exact_boundary() {
        assert_eq!(
            minimal_code_addendum(ESTABLISHED_SESSION_THRESHOLD - 1),
            MINIMAL_CODE_ADDENDUM_CONTINUING
        );
        assert_eq!(
            minimal_code_addendum(ESTABLISHED_SESSION_THRESHOLD),
            MINIMAL_CODE_ADDENDUM_ESTABLISHED
        );
    }

    #[test]
    fn all_three_variants_keep_the_safety_exemption_and_example_carve_out() {
        for addendum in [
            minimal_code_addendum(0),
            minimal_code_addendum(1),
            minimal_code_addendum(ESTABLISHED_SESSION_THRESHOLD),
        ] {
            assert!(addendum.contains("never skipped for brevity"));
            assert!(addendum.contains("give me code for X"));
        }
    }

    /// The regression this function exists for: `coding` contains no `code`, so the old
    /// substring match classified the most natural name in the domain as not-a-coding-agent.
    #[test]
    fn coding_is_detected_even_though_it_contains_no_code() {
        assert!(!"coding".contains("code"), "premise of this test");
        for wording in [
            "coding",
            "coding-assistant",
            "Coding Assistant",
            "coder",
            "code-edit",
            "Code Editing",
            "programming",
            "programmer",
            "software engineering",
            "software development",
            "refactoring",
            "debugging",
            "bug fixing",
        ] {
            assert!(mentions_coding(wording), "should detect `{wording}`");
        }
    }

    /// The other half of the old bug: substring matching fired on words that merely end in
    /// "code". Whole-word prefixes reject these without needing an exclusion list.
    #[test]
    fn words_merely_ending_in_code_are_not_coding() {
        for wording in ["encode", "decode", "barcode", "geocode", "unicode"] {
            assert!(!mentions_coding(wording), "should not detect `{wording}`");
        }
    }

    /// Non-coding agents on this platform must not be offered a coding toggle.
    #[test]
    fn unrelated_agent_skills_are_not_coding() {
        for wording in [
            "weather-forecast",
            "translation",
            "business development",
            "prompt engineering",
            "transcript summary",
            "invoice processing",
        ] {
            assert!(!mentions_coding(wording), "should not detect `{wording}`");
        }
    }

    /// The ladder reaches any agent whose card declares a `%code%` skill — including a
    /// third party's, written in a language nobody here chose, exposing tools nobody here
    /// named. Text that instructs it to call `search_code` or to avoid setting up a *Cargo*
    /// project is not merely useless there: it is ~100 tokens per turn of advice addressed
    /// to a different agent, and nothing at the injection seam detects the mismatch. A test
    /// rather than a review note, because the failure is silent at every layer.
    #[test]
    fn no_variant_names_a_specific_agents_tools_or_language() {
        const AGENT_SPECIFIC: [&str; 6] = [
            "search_code",
            "list_directory",
            "read_file",
            "Cargo",
            "cargo",
            "rustc",
        ];
        for addendum in [
            minimal_code_addendum(0),
            minimal_code_addendum(1),
            minimal_code_addendum(ESTABLISHED_SESSION_THRESHOLD),
        ] {
            for needle in AGENT_SPECIFIC {
                assert!(
                    !addendum.contains(needle),
                    "ladder text names `{needle}`, which only exists in one agent"
                );
            }
        }
    }

    #[test]
    fn self_review_requires_ladder_on() {
        assert!(!wants_self_review(false, true, "did something"));
    }

    #[test]
    fn self_review_requires_wrote_code() {
        assert!(!wants_self_review(true, false, "did something"));
    }

    #[test]
    fn self_review_requires_nonempty_answer() {
        assert!(!wants_self_review(true, true, ""));
    }

    #[test]
    fn self_review_fires_when_all_conditions_met() {
        assert!(wants_self_review(true, true, "did something"));
    }
}
