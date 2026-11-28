//! The reviewer prompt and the model's JSON reply. Every model in a run gets this exact system
//! prompt, the same diff and the same touched-file context, at the router's zero-temperature
//! call (`evals::llm::complete`) — identical instructions and context across models is the
//! fairness requirement the brief asks for when comparing them.

use anyhow::{Context, Result, bail};
use nasiko_evals::prompt::ChatPrompt;

use super::api::ModelFinding;

pub const SYSTEM_PROMPT: &str = "You are a precise code reviewer. You are shown a unified diff \
and the full text of the files it touches. Report only issues you can point to a specific file \
and line for: bugs, security problems, and code-quality problems. Reply with a JSON array and \
nothing else, matching exactly this shape: [{\"file\": string, \"line\": number, \"severity\": \
\"critical\"|\"high\"|\"medium\"|\"low\", \"category\": \"bug\"|\"security\"|\"quality\", \
\"description\": string, \"confidence\": number between 0 and 1}]. A diff with no issues is a \
reason to reply with an empty array `[]`, never a fabricated finding.";

pub fn build(diff: &str, files: &std::collections::BTreeMap<String, String>) -> ChatPrompt {
    let mut user = String::new();
    user.push_str("## Diff\n\n```diff\n");
    user.push_str(diff);
    user.push_str("\n```\n");
    if !files.is_empty() {
        user.push_str("\n## Touched files at the pull request's head commit\n");
        for (path, text) in files {
            user.push_str(&format!("\n### {path}\n\n```\n{text}\n```\n"));
        }
    }
    ChatPrompt {
        system: SYSTEM_PROMPT.to_string(),
        user,
    }
}

/// The model's reply, which may wrap the JSON array in prose or a fenced code block — only the
/// outermost `[...]` is parsed.
pub fn parse_findings(reply: &str) -> Result<Vec<ModelFinding>> {
    let start = reply.find('[').context("the reply has no JSON array")?;
    let end = reply.rfind(']').context("the reply has no JSON array")?;
    if end < start {
        bail!("the reply's `[` and `]` are out of order");
    }
    serde_json::from_str(&reply[start..=end])
        .context("the reply's JSON array does not match the findings shape")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_array_parses_to_no_findings() {
        assert_eq!(parse_findings("[]").unwrap(), Vec::new());
    }

    #[test]
    fn a_fenced_reply_is_unwrapped() {
        let reply = "Here you go:\n```json\n[{\"file\": \"a.rs\", \"line\": 1, \
            \"severity\": \"low\", \"category\": \"quality\", \"description\": \"d\", \
            \"confidence\": 0.5}]\n```\nDone.";
        let findings = parse_findings(reply).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "a.rs");
    }

    #[test]
    fn prose_with_no_array_is_an_error() {
        assert!(parse_findings("I found nothing to report.").is_err());
    }

    #[test]
    fn a_malformed_entry_is_an_error_not_a_silent_drop() {
        let reply = "[{\"file\": \"a.rs\"}]";
        assert!(parse_findings(reply).is_err());
    }
}
