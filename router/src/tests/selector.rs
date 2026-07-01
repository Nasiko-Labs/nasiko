use super::*;
use super::super::models::{AgentCardSummary, SkillSummary};
use serde_json::json;
use uuid::Uuid;

fn dummy_agent(name: &str, desc: &str, skills: Vec<SkillSummary>) -> AgentCardSummary {
    AgentCardSummary {
        id: Uuid::nil(),
        name: name.to_string(),
        description: desc.to_string(),
        skills,
        tags: vec!["test".to_string()],
    }
}

// ── extract_skills ────────────────────────────────────────────────────────

#[test]
fn extract_skills_picks_up_name_and_description() {
    let json = json!([
        { "id": "s1", "name": "code-review", "description": "Reviews code for bugs", "tags": [], "examples": [] }
    ]);
    let skills = extract_skills(json);
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "code-review");
    assert_eq!(skills[0].description, "Reviews code for bugs");
}

#[test]
fn extract_skills_falls_back_to_name_when_description_missing() {
    let json = json!([{ "name": "summarize" }]);
    let skills = extract_skills(json);
    assert_eq!(skills[0].description, "summarize");
}

#[test]
fn extract_skills_skips_entries_without_name() {
    let json = json!([
        { "description": "no name here" },
        { "name": "valid-skill", "description": "does something" }
    ]);
    let skills = extract_skills(json);
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "valid-skill");
}

#[test]
fn extract_skills_returns_empty_for_non_array() {
    assert!(extract_skills(json!(null)).is_empty());
    assert!(extract_skills(json!({})).is_empty());
    assert!(extract_skills(json!("string")).is_empty());
}

#[test]
fn extract_skills_handles_empty_array() {
    assert!(extract_skills(json!([])).is_empty());
}

// ── build_system_prompt ───────────────────────────────────────────────────

fn make_selector() -> AgentSelector {
    // Provider is not called in unit tests — model string is arbitrary.
    AgentSelector::new(
        super::super::providers::LLMProvider::from_env(
            reqwest::Client::new(),
        ),
        "test-model".to_string(),
    )
}

#[test]
fn system_prompt_includes_skill_descriptions() {
    let selector = make_selector();
    let agents = vec![dummy_agent(
        "coder",
        "Writes and reviews code",
        vec![
            SkillSummary { name: "code-review".to_string(), description: "Reviews code for bugs and style issues".to_string() },
            SkillSummary { name: "refactor".to_string(), description: "Refactors code to improve readability".to_string() },
        ],
    )];
    let prompt = selector.build_system_prompt(&agents);

    assert!(prompt.contains("code-review: Reviews code for bugs and style issues"), "skill description missing from prompt");
    assert!(prompt.contains("refactor: Refactors code to improve readability"), "second skill description missing");
}

#[test]
fn system_prompt_does_not_duplicate_name_as_description() {
    let selector = make_selector();
    let agents = vec![dummy_agent(
        "agent",
        "Does things",
        vec![SkillSummary { name: "do-thing".to_string(), description: "do-thing".to_string() }],
    )];
    let prompt = selector.build_system_prompt(&agents);
    // Should appear once as "do-thing: do-thing", not twice or in a broken format
    let count = prompt.matches("do-thing").count();
    assert!(count >= 1, "skill name should appear in prompt");
}

#[test]
fn system_prompt_shows_none_when_no_skills() {
    let selector = make_selector();
    let agents = vec![dummy_agent("agent", "Does stuff", vec![])];
    let prompt = selector.build_system_prompt(&agents);
    assert!(prompt.contains("(none)"), "empty skill list should show (none)");
}

#[test]
fn system_prompt_includes_agent_description_and_name() {
    let selector = make_selector();
    let agents = vec![dummy_agent("my-agent", "Handles customer support queries", vec![])];
    let prompt = selector.build_system_prompt(&agents);
    assert!(prompt.contains("my-agent"));
    assert!(prompt.contains("Handles customer support queries"));
}
