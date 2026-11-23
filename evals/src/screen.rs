//! The Verified-style keep/drop rule over reviewer verdicts (prompt P3).
//!
//! As in SWE-bench Verified, the worst score across reviewers counts: a task is kept only if no
//! reviewer scored clarity or test validity 2 or more and nobody flagged another problem. A
//! person is asked only when no reviewer could give a verdict at all.

use serde::{Deserialize, Serialize};

use crate::instance::Difficulty;
use crate::prompt::ScreenVerdict;

/// Highest score (on either 0-3 scale) a kept task may have.
pub const MAX_KEPT_SCORE: u8 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum ScreenDecision {
    Accept {
        difficulty: Difficulty,
    },
    Reject {
        reason: String,
    },
    /// No reviewer verdict could be obtained; a person must decide.
    NeedsHuman,
}

/// Decides from every verdict there is. A person's verdict (reviewer `human:<name>`) overrides
/// the models'.
pub fn decide(verdicts: &[ScreenVerdict]) -> ScreenDecision {
    let human: Vec<&ScreenVerdict> = verdicts
        .iter()
        .filter(|v| v.reviewer.starts_with("human:"))
        .collect();
    let counted: Vec<&ScreenVerdict> = if human.is_empty() {
        verdicts.iter().collect()
    } else {
        human
    };
    if counted.is_empty() {
        return ScreenDecision::NeedsHuman;
    }
    let clarity = counted.iter().map(|v| v.clarity).max().unwrap_or(0);
    let validity = counted.iter().map(|v| v.test_validity).max().unwrap_or(0);
    let problems: Vec<&str> = counted
        .iter()
        .flat_map(|v| v.other_problems.iter().map(String::as_str))
        .collect();
    if clarity > MAX_KEPT_SCORE {
        return reject(format!("clarity scored {clarity}"));
    }
    if validity > MAX_KEPT_SCORE {
        return reject(format!("test validity scored {validity}"));
    }
    if !problems.is_empty() {
        return reject(format!("flagged: {}", problems.join(", ")));
    }
    let difficulty = counted
        .iter()
        .map(|v| v.effort)
        .max()
        .unwrap_or(Difficulty::Min15To1H);
    ScreenDecision::Accept { difficulty }
}

fn reject(reason: String) -> ScreenDecision {
    ScreenDecision::Reject { reason }
}

/// Easy / medium / hard as reported: Verified's effort groups, the two longest merged.
pub fn level(d: Difficulty) -> &'static str {
    match d {
        Difficulty::Under15Min => "easy",
        Difficulty::Min15To1H => "medium",
        Difficulty::H1To4 | Difficulty::Over4H => "hard",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(reviewer: &str, clarity: u8, validity: u8, effort: Difficulty) -> ScreenVerdict {
        ScreenVerdict {
            reviewer: reviewer.into(),
            clarity,
            clarity_reason: String::new(),
            test_validity: validity,
            test_validity_reason: String::new(),
            other_problems: vec![],
            effort,
            effort_reason: String::new(),
        }
    }

    #[test]
    fn worst_score_counts() {
        let d = decide(&[
            v("a", 0, 0, Difficulty::Under15Min),
            v("b", 1, 1, Difficulty::H1To4),
        ]);
        assert_eq!(
            d,
            ScreenDecision::Accept {
                difficulty: Difficulty::H1To4
            }
        );
        let d = decide(&[
            v("a", 0, 0, Difficulty::Under15Min),
            v("b", 2, 0, Difficulty::H1To4),
        ]);
        assert!(matches!(d, ScreenDecision::Reject { .. }));
    }

    #[test]
    fn a_person_is_asked_only_without_any_verdict() {
        assert_eq!(decide(&[]), ScreenDecision::NeedsHuman);
        let mut flagged = v("a", 0, 0, Difficulty::Under15Min);
        flagged.other_problems = vec!["needs_network".into()];
        assert!(matches!(
            decide(&[flagged.clone()]),
            ScreenDecision::Reject { .. }
        ));
        let person = v("human:manu", 0, 0, Difficulty::Min15To1H);
        assert_eq!(
            decide(&[flagged, person]),
            ScreenDecision::Accept {
                difficulty: Difficulty::Min15To1H
            }
        );
    }
}
