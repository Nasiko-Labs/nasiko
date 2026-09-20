use nasiko_orchestrator::AgentSelector;
use nasiko_orchestrator::models::{AgentCardSummary, SkillSummary};
use nasiko_orchestrator::providers::LLMProvider;
use uuid::Uuid;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn dummy_agent(name: &str, desc: &str, skills: Vec<SkillSummary>) -> AgentCardSummary {
    AgentCardSummary {
        id: Uuid::new_v4(),
        name: name.to_string(),
        description: desc.to_string(),
        skills,
        tags: vec!["test".to_string()],
    }
}

fn make_selector() -> AgentSelector {
    AgentSelector::new(
        LLMProvider::from_env(reqwest::Client::new()),
        "test-model".to_string(),
    )
}

// ── AgentSelector construction ────────────────────────────────────────────────

#[test]
fn agent_selector_new_does_not_panic() {
    let _ = make_selector();
}

#[test]
fn agent_selector_reports_model_name() {
    let selector = AgentSelector::new(
        LLMProvider::from_env(reqwest::Client::new()),
        "gpt-4o-mini".to_string(),
    );
    assert_eq!(selector.model_name(), "gpt-4o-mini");
}

// ── AgentCardSummary construction and serialization ───────────────────────────

#[test]
fn agent_card_summary_constructs_with_all_fields() {
    let id = Uuid::new_v4();
    let summary = AgentCardSummary {
        id,
        name: "code-agent".to_string(),
        description: "Writes code".to_string(),
        skills: vec![SkillSummary {
            name: "rust".to_string(),
            description: "Rust programming".to_string(),
        }],
        tags: vec!["engineering".to_string()],
    };
    assert_eq!(summary.id, id);
    assert_eq!(summary.skills.len(), 1);
}

#[test]
fn agent_card_summary_round_trips_through_json() {
    let summary = AgentCardSummary {
        id: Uuid::new_v4(),
        name: "agent".to_string(),
        description: "desc".to_string(),
        skills: vec![SkillSummary {
            name: "s1".to_string(),
            description: "d1".to_string(),
        }],
        tags: vec!["t1".to_string()],
    };
    let json = serde_json::to_string(&summary).unwrap();
    let restored: AgentCardSummary = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.name, summary.name);
    assert_eq!(restored.skills[0].name, "s1");
}

#[test]
fn agent_card_summary_with_empty_skills() {
    let summary = dummy_agent("bare-agent", "Does stuff", vec![]);
    assert!(summary.skills.is_empty());
    assert_eq!(summary.description, "Does stuff");
}

// ── SkillSummary ──────────────────────────────────────────────────────────────

#[test]
fn skill_summary_constructs() {
    let s = SkillSummary {
        name: "code-review".to_string(),
        description: "Reviews code for bugs".to_string(),
    };
    assert_eq!(s.name, "code-review");
    assert_eq!(s.description, "Reviews code for bugs");
}

#[test]
fn skill_summary_round_trips_through_json() {
    let original = SkillSummary {
        name: "summarize".to_string(),
        description: "Summarizes long documents".to_string(),
    };
    let json = serde_json::to_string(&original).unwrap();
    let restored: SkillSummary = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.name, original.name);
    assert_eq!(restored.description, original.description);
}

// ── select_agent: no agents → error ──────────────────────────────────────────

#[tokio::test]
async fn select_agent_with_empty_list_returns_error() {
    let selector = make_selector();
    let result = selector.select_agent("some query", &[], &[]).await;
    assert!(
        result.is_err(),
        "select_agent should return Err when no agents provided"
    );
}

// ── select_agent: live LLM tests ──────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires live OpenAI-compatible LLM API"]
async fn select_agent_with_live_llm_returns_valid_selection() {
    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY required");
    let base_url =
        std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com".into());

    let provider = LLMProvider::new(reqwest::Client::new(), api_key, base_url);
    let selector = AgentSelector::new(provider, "gpt-4o-mini".to_string());

    let agents = vec![
        AgentCardSummary {
            id: Uuid::new_v4(),
            name: "coding-agent".to_string(),
            description: "Writes and reviews Rust code".to_string(),
            skills: vec![SkillSummary {
                name: "rust".to_string(),
                description: "Rust programming".to_string(),
            }],
            tags: vec!["engineering".to_string()],
        },
        AgentCardSummary {
            id: Uuid::new_v4(),
            name: "finance-agent".to_string(),
            description: "Analyzes stock prices and crypto markets".to_string(),
            skills: vec![SkillSummary {
                name: "trading".to_string(),
                description: "Financial analysis".to_string(),
            }],
            tags: vec!["finance".to_string()],
        },
    ];

    let result = selector
        .select_agent("write a Rust function", &[], &agents)
        .await;
    assert!(result.is_ok(), "expected Ok, got {result:?}");
    let (selection, _usage) = result.unwrap();
    assert!(!selection.reasoning.is_empty());
}

#[tokio::test]
#[ignore = "requires live DB"]
async fn fetch_active_agents_from_db() {
    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost/nasiko".to_string());
    let pool = sqlx::PgPool::connect(&db_url).await.unwrap();
    let result = AgentSelector::fetch_active_agents(&pool).await;
    assert!(result.is_ok());
}
