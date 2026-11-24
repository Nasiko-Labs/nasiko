//! Which agents may be shared.
//!
//! A harness agent belongs to one user. It is never made public, no `agent_grants` row is ever
//! created for it, and a grant row that exists anyway (written straight to the database) confers
//! nothing: every access check and listing ANDs [`grants_reach_sql`] onto its grant branches, so a
//! harness agent is seen by its owner or a superuser only (the share routes refuse to make one
//! public, so its `is_public` stays false), and talked to by its owner only: a superuser opens or
//! resumes no session on someone else's. Making one private is always allowed.

/// `agents.workload_kind` of a harness agent.
pub const HARNESS_WORKLOAD_KIND: &str = "harness";

/// Whether an agent of `workload_kind` may be shared: made public, or granted to a user, a unit,
/// the organization or another agent.
pub fn is_shareable(workload_kind: &str) -> bool {
    workload_kind != HARNESS_WORKLOAD_KIND
}

/// SQL condition, true when a grant may reach the agent `agent_ref` names (a table name or an
/// alias in the enclosing query).
pub fn grants_reach_sql(agent_ref: &str) -> String {
    format!("{agent_ref}.workload_kind <> '{HARNESS_WORKLOAD_KIND}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_harness_agent_is_not_shareable() {
        assert!(!is_shareable(HARNESS_WORKLOAD_KIND));
    }

    #[test]
    fn every_other_kind_is_shareable() {
        for kind in ["agent", "mcp_connector", "acp_coding_agent"] {
            assert!(is_shareable(kind), "{kind}");
        }
    }

    #[test]
    fn the_sql_condition_names_the_given_agent_reference() {
        assert_eq!(grants_reach_sql("a"), "a.workload_kind <> 'harness'");
    }
}
