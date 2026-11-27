//! A user's own harness agent: the `agents` row every session that user opens on one harness
//! runs as. It is owned by the user, so the model router and the MCP gateway resolve the user's
//! own settings for it and its time is metered as theirs. The server creates it on the user's
//! first session of that harness; a sandbox provider that runs sessions inside it finds it by
//! [`name`].

/// A user's harness agent is `harness-<harness id>`.
pub const NAME_PREFIX: &str = "harness-";

/// The name of the user's harness agent for `harness_id`, unique per user.
pub fn name(harness_id: &str) -> String {
    format!("{NAME_PREFIX}{harness_id}")
}

/// The display name of the user's harness agent for the harness called `harness_name`.
pub fn display_name(harness_name: &str) -> String {
    format!("{harness_name} sessions")
}

/// The description of the user's harness agent for the harness called `harness_name`.
pub fn description(harness_name: &str) -> String {
    format!(
        "Runs your {harness_name} sessions with your own model settings and tools. \
         Created by the platform and never shared."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_users_harness_agent_is_named_after_its_harness() {
        assert_eq!(name("claude-code"), "harness-claude-code");
        assert_eq!(display_name("OpenCode"), "OpenCode sessions");
        assert!(description("Codex").contains("Codex sessions"));
    }
}
