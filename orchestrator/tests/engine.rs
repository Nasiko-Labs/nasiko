use nasiko_orchestrator::{OssRoutingEngine, RouterConfig};
use reqwest::Client;

// ── RouterConfig defaults ─────────────────────────────────────────────────────

#[test]
fn router_config_defaults_are_sensible() {
    let cfg = RouterConfig::default();
    assert_eq!(cfg.shortlist_threshold, 15);
    assert_eq!(cfg.shortlist_size, 10);
    assert_eq!(cfg.max_history_messages, 20);
}

#[test]
fn router_config_custom_values() {
    let cfg = RouterConfig {
        shortlist_threshold: 5,
        shortlist_size: 3,
        max_history_messages: 10,
    };
    assert_eq!(cfg.shortlist_threshold, 5);
    assert_eq!(cfg.shortlist_size, 3);
    assert_eq!(cfg.max_history_messages, 10);
}

// ── OssRoutingEngine construction ─────────────────────────────────────────────

#[test]
fn oss_routing_engine_new_does_not_panic() {
    let _ = OssRoutingEngine::new(
        RouterConfig::default(),
        Client::new(),
        String::new(),
        "https://api.openai.com".to_string(),
        "gpt-4o".to_string(),
        "text-embedding-3-small".to_string(),
    );
}

#[test]
fn oss_routing_engine_new_with_empty_api_key() {
    // Should construct fine — Stage 1 will be disabled at runtime
    let _ = OssRoutingEngine::new(
        RouterConfig::default(),
        Client::new(),
        String::new(),
        "https://api.openai.com".to_string(),
        "gpt-4o-mini".to_string(),
        "nomic-embed-text".to_string(),
    );
}

#[test]
fn oss_routing_engine_new_with_custom_config() {
    let config = RouterConfig {
        shortlist_threshold: 20,
        shortlist_size: 5,
        max_history_messages: 15,
    };
    let _ = OssRoutingEngine::new(
        config,
        Client::new(),
        "sk-test".to_string(),
        "https://custom-llm.example.com".to_string(),
        "mixtral-8x7b".to_string(),
        "nomic-embed-text".to_string(),
    );
}

// ── route: live DB/LLM tests ──────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires live DB and LLM API"]
async fn route_returns_error_when_no_agents_in_db() {
    use nasiko_orchestrator::{RouteRequest, RoutingEngine};

    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost/nasiko".to_string());
    let pool = sqlx::PgPool::connect(&db_url).await.unwrap();

    let engine = OssRoutingEngine::new(
        RouterConfig::default(),
        Client::new(),
        String::new(),
        "https://api.openai.com".to_string(),
        "gpt-4o".to_string(),
        "text-embedding-3-small".to_string(),
    );

    let req = RouteRequest {
        query: "test query".to_string(),
        session_id: "test-session".to_string(),
        user_id: uuid::Uuid::new_v4(),
        file_parts: vec![],
    };

    // With no running agents in DB, should return NoAgentsAvailable
    let result = engine.route(req, &pool).await;
    // Either succeeds (if agents exist) or fails with NoAgentsAvailable
    match result {
        Ok(_) | Err(nasiko_orchestrator::RouterError::NoAgentsAvailable) => {}
        Err(e) => panic!("unexpected error: {e}"),
    }
}
